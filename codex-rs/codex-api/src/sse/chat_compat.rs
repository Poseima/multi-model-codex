use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::sse::chat_compat_fork::ContentSegment;
use crate::sse::chat_compat_fork::ThinkTagStreamSplitter;
use crate::telemetry::SseTelemetry;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::Stream;
use futures::StreamExt;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

pub(crate) use crate::sse::chat_compat_fork::ChatReasoningFormat;

pub(crate) fn spawn_chat_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    reasoning_format: ChatReasoningFormat,
    _turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_chat_sse_with_format(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            reasoning_format,
        )
        .await;
    });
    ResponseStream { rx_event }
}

/// Processes Server-Sent Events from the Chat Completions streaming API.
///
/// Handles `data: [DONE]` and `data: DONE` sentinels, tool call accumulation,
/// reasoning extraction, and all finish reason semantics.
pub async fn process_chat_sse<S>(
    stream: S,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<std::sync::Arc<dyn SseTelemetry>>,
) where
    S: Stream<Item = Result<bytes::Bytes, codex_client::TransportError>> + Unpin,
{
    process_chat_sse_with_format(
        stream,
        tx_event,
        idle_timeout,
        telemetry,
        ChatReasoningFormat::Standard,
    )
    .await;
}

async fn process_chat_sse_with_format<S>(
    stream: S,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<std::sync::Arc<dyn SseTelemetry>>,
    reasoning_format: ChatReasoningFormat,
) where
    S: Stream<Item = Result<bytes::Bytes, codex_client::TransportError>> + Unpin,
{
    let mut stream = stream.eventsource();

    #[derive(Default, Debug)]
    struct ToolCallState {
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    }

    let mut tool_calls: HashMap<usize, ToolCallState> = HashMap::new();
    let mut tool_call_order: Vec<usize> = Vec::new();
    let mut tool_call_order_seen: HashSet<usize> = HashSet::new();
    let mut tool_call_index_by_id: HashMap<String, usize> = HashMap::new();
    let mut next_tool_call_index = 0usize;
    let mut last_tool_call_index: Option<usize> = None;
    let mut assistant_item: Option<ResponseItem> = None;
    let mut reasoning_item: Option<ResponseItem> = None;
    let mut content_splitter = ThinkTagStreamSplitter::new(reasoning_format);
    let mut token_usage: Option<TokenUsage> = None;

    async fn flush_and_complete(
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
        content_splitter: &mut ThinkTagStreamSplitter,
        reasoning_item: &mut Option<ResponseItem>,
        assistant_item: &mut Option<ResponseItem>,
        token_usage: Option<TokenUsage>,
    ) {
        append_content_segments(
            tx_event,
            assistant_item,
            reasoning_item,
            content_splitter.flush_remaining(),
        )
        .await;

        if let Some(reasoning) = reasoning_item.take() {
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemDone(reasoning)))
                .await;
        }

        if let Some(assistant) = assistant_item.take() {
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemDone(assistant)))
                .await;
        }

        let _ = tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: String::new(),
                token_usage,
            }))
            .await;
    }

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                flush_and_complete(
                    &tx_event,
                    &mut content_splitter,
                    &mut reasoning_item,
                    &mut assistant_item,
                    token_usage.take(),
                )
                .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("SSE event: {}", sse.data);

        let data = sse.data.trim();

        if data.is_empty() {
            continue;
        }

        if data == "[DONE]" || data == "DONE" {
            flush_and_complete(
                &tx_event,
                &mut content_splitter,
                &mut reasoning_item,
                &mut assistant_item,
                token_usage.take(),
            )
            .await;
            return;
        }

        let value: serde_json::Value = match serde_json::from_str(data) {
            Ok(val) => val,
            Err(err) => {
                debug!(
                    "Failed to parse ChatCompletions SSE event: {err}, data: {}",
                    data
                );
                continue;
            }
        };

        if let Some(usage_val) = value.get("usage") {
            token_usage = parse_chat_usage(usage_val);
        }

        let Some(choices) = value.get("choices").and_then(|c| c.as_array()) else {
            continue;
        };

        for choice in choices {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning") {
                    if let Some(text) = reasoning.as_str() {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    } else if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    } else if let Some(text) = reasoning.get("content").and_then(|v| v.as_str()) {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    }
                }

                if let Some(content) = delta.get("content") {
                    if content.is_array() {
                        for item in content.as_array().unwrap_or(&vec![]) {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                append_content_segments(
                                    &tx_event,
                                    &mut assistant_item,
                                    &mut reasoning_item,
                                    content_splitter.split_chunk(text),
                                )
                                .await;
                            }
                        }
                    } else if let Some(text) = content.as_str() {
                        append_content_segments(
                            &tx_event,
                            &mut assistant_item,
                            &mut reasoning_item,
                            content_splitter.split_chunk(text),
                        )
                        .await;
                    }
                }

                if let Some(tool_call_values) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for tool_call in tool_call_values {
                        let mut index = tool_call
                            .get("index")
                            .and_then(serde_json::Value::as_u64)
                            .map(|i| i as usize);

                        let mut call_id_for_lookup = None;
                        if let Some(call_id) = tool_call.get("id").and_then(|i| i.as_str()) {
                            call_id_for_lookup = Some(call_id.to_string());
                            if let Some(existing) = tool_call_index_by_id.get(call_id) {
                                index = Some(*existing);
                            }
                        }

                        if index.is_none() && call_id_for_lookup.is_none() {
                            index = last_tool_call_index;
                        }

                        let index = index.unwrap_or_else(|| {
                            while tool_calls.contains_key(&next_tool_call_index) {
                                next_tool_call_index += 1;
                            }
                            let idx = next_tool_call_index;
                            next_tool_call_index += 1;
                            idx
                        });

                        let call_state = tool_calls.entry(index).or_default();
                        if tool_call_order_seen.insert(index) {
                            tool_call_order.push(index);
                        }

                        if let Some(id) = tool_call.get("id").and_then(|i| i.as_str()) {
                            call_state.id.get_or_insert_with(|| id.to_string());
                            tool_call_index_by_id.entry(id.to_string()).or_insert(index);
                        }

                        if let Some(func) = tool_call.get("function") {
                            if let Some(fname) = func.get("name").and_then(|n| n.as_str())
                                && !fname.is_empty()
                            {
                                call_state.name.get_or_insert_with(|| fname.to_string());
                            }
                            if let Some(arguments) = func.get("arguments").and_then(|a| a.as_str())
                            {
                                call_state.arguments.push_str(arguments);
                            }
                        }

                        last_tool_call_index = Some(index);
                    }
                }
            }

            if let Some(message) = choice.get("message")
                && let Some(reasoning) = message.get("reasoning")
            {
                if let Some(text) = reasoning.as_str() {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                } else if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                } else if let Some(text) = reasoning.get("content").and_then(|v| v.as_str()) {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                }
            }

            let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());
            if finish_reason == Some("stop") {
                append_content_segments(
                    &tx_event,
                    &mut assistant_item,
                    &mut reasoning_item,
                    content_splitter.flush_remaining(),
                )
                .await;

                if let Some(reasoning) = reasoning_item.take() {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(reasoning)))
                        .await;
                }

                if let Some(assistant) = assistant_item.take() {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(assistant)))
                        .await;
                }

                // Some providers (e.g. MiniMax) use finish_reason "stop" even
                // when tool calls are present. Emit any accumulated tool calls
                // so they are not silently dropped.
                for index in tool_call_order.drain(..) {
                    let Some(state) = tool_calls.remove(&index) else {
                        continue;
                    };
                    tool_call_order_seen.remove(&index);
                    let ToolCallState {
                        id,
                        name,
                        arguments,
                    } = state;
                    let Some(name) = name else {
                        debug!("Skipping tool call at index {index} because name is missing");
                        continue;
                    };
                    let item = ResponseItem::FunctionCall {
                        id: None,
                        name,
                        arguments,
                        call_id: id.unwrap_or_else(|| format!("tool-call-{index}")),
                    };
                    let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                }

                // Don't send Completed here — the usage chunk arrives after
                // stop but before [DONE]. Let the [DONE]/stream-end path
                // send Completed with the accumulated token_usage.
                continue;
            }

            if finish_reason == Some("length") {
                let _ = tx_event.send(Err(ApiError::ContextWindowExceeded)).await;
                return;
            }

            if finish_reason == Some("tool_calls") {
                append_content_segments(
                    &tx_event,
                    &mut assistant_item,
                    &mut reasoning_item,
                    content_splitter.flush_remaining(),
                )
                .await;

                if let Some(reasoning) = reasoning_item.take() {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(reasoning)))
                        .await;
                }

                for index in tool_call_order.drain(..) {
                    let Some(state) = tool_calls.remove(&index) else {
                        continue;
                    };
                    tool_call_order_seen.remove(&index);
                    let ToolCallState {
                        id,
                        name,
                        arguments,
                    } = state;
                    let Some(name) = name else {
                        debug!("Skipping tool call at index {index} because name is missing");
                        continue;
                    };
                    let item = ResponseItem::FunctionCall {
                        id: None,
                        name,
                        arguments,
                        call_id: id.unwrap_or_else(|| format!("tool-call-{index}")),
                    };
                    let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                }
            }
        }
    }
}

async fn append_content_segments(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    assistant_item: &mut Option<ResponseItem>,
    reasoning_item: &mut Option<ResponseItem>,
    segments: Vec<ContentSegment>,
) {
    for segment in segments {
        match segment {
            ContentSegment::Assistant(text) => {
                append_assistant_text(tx_event, assistant_item, text).await;
            }
            ContentSegment::Reasoning(text) => {
                append_reasoning_text(tx_event, reasoning_item, text).await;
            }
        }
    }
}

async fn append_assistant_text(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    assistant_item: &mut Option<ResponseItem>,
    text: String,
) {
    if assistant_item.is_none() {
        let item = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![],
            end_turn: None,
            phase: None,
        };
        *assistant_item = Some(item.clone());
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(item)))
            .await;
    }

    if let Some(ResponseItem::Message { content, .. }) = assistant_item {
        if let Some(ContentItem::OutputText { text: existing }) = content.last_mut() {
            existing.push_str(&text);
        } else {
            content.push(ContentItem::OutputText { text: text.clone() });
        }
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(text.clone())))
            .await;
    }
}

async fn append_reasoning_text(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    reasoning_item: &mut Option<ResponseItem>,
    text: String,
) {
    if reasoning_item.is_none() {
        let item = ResponseItem::Reasoning {
            id: String::new(),
            summary: Vec::new(),
            content: Some(vec![]),
            encrypted_content: None,
        };
        *reasoning_item = Some(item.clone());
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(item)))
            .await;
    }

    if let Some(ResponseItem::Reasoning {
        content: Some(content),
        ..
    }) = reasoning_item
    {
        let content_index = if let Some(last_entry) = content.last_mut() {
            match last_entry {
                ReasoningItemContent::ReasoningText {
                    text: existing_text,
                }
                | ReasoningItemContent::Text {
                    text: existing_text,
                } => existing_text.push_str(&text),
            }
            (content.len() - 1) as i64
        } else {
            content.push(ReasoningItemContent::ReasoningText { text: text.clone() });
            0
        };

        let _ = tx_event
            .send(Ok(ResponseEvent::ReasoningContentDelta {
                delta: text.clone(),
                content_index,
            }))
            .await;
    }
}

/// Parse the `usage` object from a Chat Completions SSE chunk into `TokenUsage`.
///
/// Expected shape (OpenAI / OpenRouter / MiniMax):
/// ```json
/// {
///   "prompt_tokens": 10,
///   "completion_tokens": 20,
///   "total_tokens": 30,
///   "completion_tokens_details": { "reasoning_tokens": 5 }
/// }
/// ```
fn parse_chat_usage(usage: &serde_json::Value) -> Option<TokenUsage> {
    let prompt_tokens = usage.get("prompt_tokens")?.as_i64()?;
    let completion_tokens = usage.get("completion_tokens")?.as_i64()?;
    let total_tokens = usage
        .get("total_tokens")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let reasoning_output_tokens = usage
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let cached_input_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens: prompt_tokens,
        cached_input_tokens,
        output_tokens: completion_tokens,
        reasoning_output_tokens,
        total_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_protocol::models::ResponseItem;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::io::ReaderStream;

    fn build_body(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for e in events {
            body.push_str(&format!("event: message\ndata: {e}\n\n"));
        }
        body
    }

    /// Regression test: the stream should complete when we see a `[DONE]` sentinel.
    #[tokio::test]
    async fn completes_on_done_sentinel_without_json() {
        let events = collect_events("event: message\ndata: [DONE]\n\n").await;
        assert_matches!(&events[..], [ResponseEvent::Completed { .. }]);
    }

    async fn collect_events(body: &str) -> Vec<ResponseEvent> {
        collect_events_with_format(body, ChatReasoningFormat::Standard).await
    }

    async fn collect_events_with_format(
        body: &str,
        reasoning_format: ChatReasoningFormat,
    ) -> Vec<ResponseEvent> {
        let reader = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map_err(|err| codex_client::TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        tokio::spawn(process_chat_sse_with_format(
            reader,
            tx,
            Duration::from_millis(1000),
            None,
            reasoning_format,
        ));

        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev.expect("stream error"));
        }
        out
    }

    fn assistant_text_deltas(events: &[ResponseEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|ev| match ev {
                ResponseEvent::OutputTextDelta(delta) => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    fn reasoning_text_deltas(events: &[ResponseEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|ev| match ev {
                ResponseEvent::ReasoningContentDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    fn assistant_output_text_parts(events: &[ResponseEvent]) -> Vec<Vec<String>> {
        events
            .iter()
            .filter_map(|ev| match ev {
                ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. })
                    if role == "assistant" =>
                {
                    Some(
                        content
                            .iter()
                            .filter_map(|item| match item {
                                ContentItem::OutputText { text } => Some(text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                    )
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn concatenates_tool_call_arguments_across_deltas() {
        let delta_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "index": 0,
                        "function": { "name": "do_a" }
                    }]
                }
            }]
        });

        let delta_args_1 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{ \"foo\":" }
                    }]
                }
            }]
        });

        let delta_args_2 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "1}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_name, delta_args_1, delta_args_2, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if call_id == "call_a" && name == "do_a" && arguments == "{ \"foo\":1}"
        );
    }

    #[tokio::test]
    async fn emits_multiple_tool_calls() {
        let delta_a = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{\"foo\":1}" }
                    }]
                }
            }]
        });

        let delta_b = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_b",
                        "function": { "name": "do_b", "arguments": "{\"bar\":2}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_a, delta_b, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_a, name: name_a, arguments: args_a, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_b, name: name_b, arguments: args_b, .. }),
                ResponseEvent::Completed { .. }
            ] if call_a == "call_a" && name_a == "do_a" && args_a == "{\"foo\":1}" && call_b == "call_b" && name_b == "do_b" && args_b == "{\"bar\":2}"
        );
    }

    #[tokio::test]
    async fn emits_tool_calls_for_multiple_choices() {
        let payload = json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [{
                            "id": "call_a",
                            "index": 0,
                            "function": { "name": "do_a", "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                },
                {
                    "delta": {
                        "tool_calls": [{
                            "id": "call_b",
                            "index": 0,
                            "function": { "name": "do_b", "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let body = build_body(&[payload]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_a, name: name_a, arguments: args_a, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_b, name: name_b, arguments: args_b, .. }),
                ResponseEvent::Completed { .. }
            ] if call_a == "call_a" && name_a == "do_a" && args_a == "{}" && call_b == "call_b" && name_b == "do_b" && args_b == "{}"
        );
    }

    #[tokio::test]
    async fn merges_tool_calls_by_index_when_id_missing_on_subsequent_deltas() {
        let delta_with_id = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{ \"foo\":" }
                    }]
                }
            }]
        });

        let delta_without_id = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "1}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_with_id, delta_without_id, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if call_id == "call_a" && name == "do_a" && arguments == "{ \"foo\":1}"
        );
    }

    #[tokio::test]
    async fn preserves_tool_call_name_when_empty_deltas_arrive() {
        let delta_with_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a" }
                    }]
                }
            }]
        });

        let delta_with_empty_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "", "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_with_name, delta_with_empty_name, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if name == "do_a" && arguments == "{}"
        );
    }

    #[tokio::test]
    async fn emits_tool_calls_even_when_content_and_reasoning_present() {
        let delta_content_and_tools = json!({
            "choices": [{
                "delta": {
                    "content": [{"text": "hi"}],
                    "reasoning": "because",
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_content_and_tools, finish]);
        let events = collect_events(&body).await;

        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. }),
                ResponseEvent::ReasoningContentDelta { .. },
                ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }),
                ResponseEvent::OutputTextDelta(delta),
                ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::Message { .. }),
                ResponseEvent::Completed { .. }
            ] if delta == "hi" && call_id == "call_a" && name == "do_a"
        );
    }

    #[tokio::test]
    async fn minimax_think_tags_are_split_from_content() {
        let delta = json!({
            "choices": [{
                "delta": {
                    "content": "<think>internal</think>visible"
                }
            }]
        });
        let finish = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        let body = build_body(&[delta, finish]);

        let events = collect_events_with_format(&body, ChatReasoningFormat::MinimaxThinkTags).await;
        assert_eq!(assistant_text_deltas(&events), vec!["visible".to_string()]);
        assert_eq!(reasoning_text_deltas(&events), vec!["internal".to_string()]);
    }

    #[tokio::test]
    async fn minimax_think_tags_split_across_chunks_are_handled() {
        let chunk_1 = json!({
            "choices": [{
                "delta": {
                    "content": "<th"
                }
            }]
        });
        let chunk_2 = json!({
            "choices": [{
                "delta": {
                    "content": "ink>alpha</thi"
                }
            }]
        });
        let chunk_3 = json!({
            "choices": [{
                "delta": {
                    "content": "nk>done"
                }
            }]
        });
        let finish = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        let body = build_body(&[chunk_1, chunk_2, chunk_3, finish]);

        let events = collect_events_with_format(&body, ChatReasoningFormat::MinimaxThinkTags).await;
        assert_eq!(assistant_text_deltas(&events), vec!["done".to_string()]);
        assert_eq!(reasoning_text_deltas(&events), vec!["alpha".to_string()]);
    }

    #[tokio::test]
    async fn standard_mode_keeps_think_tags_in_assistant_content() {
        let delta = json!({
            "choices": [{
                "delta": {
                    "content": "<think>x</think>y"
                }
            }]
        });
        let finish = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        let body = build_body(&[delta, finish]);

        let events = collect_events_with_format(&body, ChatReasoningFormat::Standard).await;
        assert_eq!(
            assistant_text_deltas(&events),
            vec!["<think>x</think>y".to_string()]
        );
        assert_eq!(reasoning_text_deltas(&events), Vec::<String>::new());
    }

    #[tokio::test]
    async fn coalesces_assistant_output_item_content_when_streaming_multiple_chunks() {
        let chunk_1 = json!({
            "choices": [{
                "delta": {
                    "content": "Hey! What "
                }
            }]
        });
        let chunk_2 = json!({
            "choices": [{
                "delta": {
                    "content": "are you working on today?"
                }
            }]
        });
        let finish = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        let body = build_body(&[chunk_1, chunk_2, finish]);

        let events = collect_events(&body).await;
        assert_eq!(
            assistant_output_text_parts(&events),
            vec![vec!["Hey! What are you working on today?".to_string()]]
        );
    }

    #[tokio::test]
    async fn coalesces_reasoning_output_item_content_when_streaming_multiple_chunks() {
        let chunk_1 = json!({
            "choices": [{
                "delta": {
                    "reasoning": "thinking "
                }
            }]
        });
        let chunk_2 = json!({
            "choices": [{
                "delta": {
                    "reasoning": "more"
                }
            }]
        });
        let finish = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        let body = build_body(&[chunk_1, chunk_2, finish]);

        let events = collect_events(&body).await;
        let reasoning_contents = events
            .iter()
            .find_map(|ev| match ev {
                ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                    content: Some(content),
                    ..
                }) => Some(content.clone()),
                _ => None,
            })
            .expect("expected reasoning output item");
        assert_eq!(
            reasoning_contents,
            vec![ReasoningItemContent::ReasoningText {
                text: "thinking more".to_string()
            }]
        );
    }

    /// Some providers (e.g. MiniMax) use `finish_reason: "stop"` even when
    /// tool calls are present. Complete tool calls (with a name) must still be
    /// emitted so the agent loop can execute them.
    #[tokio::test]
    async fn emits_tool_calls_on_stop_finish_reason() {
        let delta_tool = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });

        let body = build_body(&[delta_tool, finish_stop]);
        let events = collect_events(&body).await;

        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if call_id == "call_a" && name == "do_a" && arguments == "{}"
        );
    }

    /// Tool calls without a name are truly partial and should be skipped.
    #[tokio::test]
    async fn drops_nameless_tool_calls_on_stop_finish_reason() {
        let delta_tool = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });

        let body = build_body(&[delta_tool, finish_stop]);
        let events = collect_events(&body).await;

        assert!(!events.iter().any(|ev| {
            matches!(
                ev,
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { .. })
            )
        }));
        assert_matches!(events.last(), Some(ResponseEvent::Completed { .. }));
    }

    /// When content and tool calls arrive together with `finish_reason: "stop"`,
    /// both the assistant message and tool calls should be emitted.
    #[tokio::test]
    async fn emits_content_and_tool_calls_on_stop_finish_reason() {
        let delta = json!({
            "choices": [{
                "delta": {
                    "content": "Let me search for that.",
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "search", "arguments": "{\"q\":\"test\"}" }
                    }]
                }
            }]
        });

        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });

        let body = build_body(&[delta, finish_stop]);
        let events = collect_events(&body).await;

        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }),
                ResponseEvent::OutputTextDelta(text),
                ResponseEvent::OutputItemDone(ResponseItem::Message { .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if text == "Let me search for that."
                && call_id == "call_a"
                && name == "search"
                && arguments == "{\"q\":\"test\"}"
        );
    }

    #[tokio::test]
    async fn extracts_token_usage_from_usage_chunk() {
        let content = json!({
            "choices": [{
                "delta": { "content": "hi" }
            }]
        });
        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });
        // Usage chunk arrives after stop, before [DONE].
        let usage_chunk = json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 42,
                "completion_tokens": 10,
                "total_tokens": 52,
                "completion_tokens_details": {
                    "reasoning_tokens": 3
                },
                "prompt_tokens_details": {
                    "cached_tokens": 5
                }
            }
        });

        let mut body = build_body(&[content, finish_stop, usage_chunk]);
        body.push_str("event: message\ndata: [DONE]\n\n");
        let events = collect_events(&body).await;

        let completed = events
            .iter()
            .find_map(|ev| match ev {
                ResponseEvent::Completed { token_usage, .. } => Some(token_usage.clone()),
                _ => None,
            })
            .expect("expected Completed event");

        assert_eq!(
            completed,
            Some(TokenUsage {
                input_tokens: 42,
                cached_input_tokens: 5,
                output_tokens: 10,
                reasoning_output_tokens: 3,
                total_tokens: 52,
            })
        );
    }

    #[tokio::test]
    async fn token_usage_is_none_when_no_usage_chunk() {
        let content = json!({
            "choices": [{
                "delta": { "content": "hi" }
            }]
        });
        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });

        let mut body = build_body(&[content, finish_stop]);
        body.push_str("event: message\ndata: [DONE]\n\n");
        let events = collect_events(&body).await;

        let completed = events
            .iter()
            .find_map(|ev| match ev {
                ResponseEvent::Completed { token_usage, .. } => Some(token_usage.clone()),
                _ => None,
            })
            .expect("expected Completed event");

        assert_eq!(completed, None);
    }
}
