use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::headers::build_conversation_headers;
use crate::requests::headers::insert_header;
use crate::requests::headers::subagent_header;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use http::HeaderMap;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use tracing::warn;

/// Assembled request body plus headers for Chat Completions streaming calls.
pub struct ChatRequest {
    pub body: Value,
    pub headers: HeaderMap,
}

pub struct ChatRequestBuilder<'a> {
    model: &'a str,
    instructions: &'a str,
    input: &'a [ResponseItem],
    tools: &'a [Value],
    conversation_id: Option<String>,
    session_source: Option<SessionSource>,
}

impl<'a> ChatRequestBuilder<'a> {
    pub fn new(
        model: &'a str,
        instructions: &'a str,
        input: &'a [ResponseItem],
        tools: &'a [Value],
    ) -> Self {
        Self {
            model,
            instructions,
            input,
            tools,
            conversation_id: None,
            session_source: None,
        }
    }

    pub fn conversation_id(mut self, id: Option<String>) -> Self {
        self.conversation_id = id;
        self
    }

    pub fn session_source(mut self, source: Option<SessionSource>) -> Self {
        self.session_source = source;
        self
    }

    pub fn build(self, provider: &Provider) -> Result<ChatRequest, ApiError> {
        // Check if the provider supports the "developer" role
        let supports_developer_role = provider.supports_developer_role();
        let system_role = provider.effective_system_role();
        let mut messages = Vec::<Value>::new();
        messages.push(json!({"role": system_role, "content": self.instructions}));

        let input = self.input;
        let mut reasoning_by_anchor_index: HashMap<usize, String> = HashMap::new();
        let mut last_emitted_role: Option<&str> = None;
        for item in input {
            match item {
                ResponseItem::Message { role, .. } => last_emitted_role = Some(role.as_str()),
                ResponseItem::FunctionCall { .. } | ResponseItem::LocalShellCall { .. } => {
                    last_emitted_role = Some("assistant")
                }
                ResponseItem::FunctionCallOutput { .. } => last_emitted_role = Some("tool"),
                ResponseItem::Reasoning { .. } | ResponseItem::Other => {}
                ResponseItem::CustomToolCall { .. } => {}
                ResponseItem::CustomToolCallOutput { .. } => {}
                ResponseItem::WebSearchCall { .. } => {}
                ResponseItem::GhostSnapshot { .. } => {}
                ResponseItem::Compaction { .. } => {}
            }
        }

        let mut last_user_index: Option<usize> = None;
        for (idx, item) in input.iter().enumerate() {
            if let ResponseItem::Message { role, .. } = item
                && role == "user"
            {
                last_user_index = Some(idx);
            }
        }

        if !matches!(last_emitted_role, Some("user")) {
            for (idx, item) in input.iter().enumerate() {
                if let Some(u_idx) = last_user_index
                    && idx <= u_idx
                {
                    continue;
                }

                if let ResponseItem::Reasoning {
                    content: Some(items),
                    ..
                } = item
                {
                    let mut text = String::new();
                    for entry in items {
                        match entry {
                            ReasoningItemContent::ReasoningText { text: segment }
                            | ReasoningItemContent::Text { text: segment } => {
                                text.push_str(segment)
                            }
                        }
                    }
                    if text.trim().is_empty() {
                        continue;
                    }

                    let mut attached = false;
                    if idx > 0
                        && let ResponseItem::Message { role, .. } = &input[idx - 1]
                        && role == "assistant"
                    {
                        reasoning_by_anchor_index
                            .entry(idx - 1)
                            .and_modify(|v| v.push_str(&text))
                            .or_insert(text.clone());
                        attached = true;
                    }

                    if !attached && idx + 1 < input.len() {
                        match &input[idx + 1] {
                            ResponseItem::FunctionCall { .. }
                            | ResponseItem::LocalShellCall { .. } => {
                                reasoning_by_anchor_index
                                    .entry(idx + 1)
                                    .and_modify(|v| v.push_str(&text))
                                    .or_insert(text.clone());
                            }
                            ResponseItem::Message { role, .. } if role == "assistant" => {
                                reasoning_by_anchor_index
                                    .entry(idx + 1)
                                    .and_modify(|v| v.push_str(&text))
                                    .or_insert(text.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let mut last_assistant_text: Option<String> = None;

        for (idx, item) in input.iter().enumerate() {
            match item {
                ResponseItem::Message { role, content, .. } => {
                    let mut text = String::new();
                    let mut items: Vec<Value> = Vec::new();
                    let mut saw_image = false;

                    for c in content {
                        match c {
                            ContentItem::InputText { text: t }
                            | ContentItem::OutputText { text: t } => {
                                text.push_str(t);
                                items.push(json!({"type":"text","text": t}));
                            }
                            ContentItem::InputImage { image_url } => {
                                saw_image = true;
                                items.push(
                                    json!({"type":"image_url","image_url": {"url": image_url}}),
                                );
                            }
                        }
                    }

                    if role == "assistant" {
                        if let Some(prev) = &last_assistant_text
                            && prev == &text
                        {
                            continue;
                        }
                        last_assistant_text = Some(text.clone());
                    }

                    let content_value = if role == "assistant" {
                        json!(text)
                    } else if saw_image {
                        json!(items)
                    } else {
                        json!(text)
                    };

                    // Convert "developer" to the provider's system role for
                    // providers that don't support the developer role.
                    let effective_role = if role == "developer" && !supports_developer_role {
                        system_role
                    } else {
                        role.as_str()
                    };
                    let mut msg = json!({"role": effective_role, "content": content_value});
                    if role == "assistant"
                        && let Some(reasoning) = reasoning_by_anchor_index.get(&idx)
                        && let Some(obj) = msg.as_object_mut()
                    {
                        obj.insert("reasoning".to_string(), json!(reasoning));
                    }
                    messages.push(msg);
                }
                ResponseItem::FunctionCall {
                    name,
                    arguments,
                    call_id,
                    ..
                } => {
                    let reasoning = reasoning_by_anchor_index.get(&idx).map(String::as_str);
                    let tool_call = json!({
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments,
                        }
                    });
                    push_tool_call_message(&mut messages, tool_call, reasoning);
                }
                ResponseItem::LocalShellCall {
                    id,
                    call_id: _,
                    status,
                    action,
                } => {
                    let reasoning = reasoning_by_anchor_index.get(&idx).map(String::as_str);
                    let tool_call = json!({
                        "id": id.clone().unwrap_or_default(),
                        "type": "local_shell_call",
                        "status": status,
                        "action": action,
                    });
                    push_tool_call_message(&mut messages, tool_call, reasoning);
                }
                ResponseItem::FunctionCallOutput { call_id, output } => {
                    let content_value = if let Some(items) = &output.content_items {
                        let mapped: Vec<Value> = items
                            .iter()
                            .map(|it| match it {
                                FunctionCallOutputContentItem::InputText { text } => {
                                    json!({"type":"text","text": text})
                                }
                                FunctionCallOutputContentItem::InputImage { image_url } => {
                                    json!({"type":"image_url","image_url": {"url": image_url}})
                                }
                            })
                            .collect();
                        json!(mapped)
                    } else {
                        json!(output.content)
                    };

                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": content_value,
                    }));
                }
                ResponseItem::CustomToolCall {
                    id,
                    call_id: _,
                    name,
                    input,
                    status: _,
                } => {
                    let tool_call = json!({
                        "id": id,
                        "type": "custom",
                        "custom": {
                            "name": name,
                            "input": input,
                        }
                    });
                    let reasoning = reasoning_by_anchor_index.get(&idx).map(String::as_str);
                    push_tool_call_message(&mut messages, tool_call, reasoning);
                }
                ResponseItem::CustomToolCallOutput { call_id, output } => {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": output,
                    }));
                }
                ResponseItem::GhostSnapshot { .. } => {
                    continue;
                }
                ResponseItem::Reasoning { .. }
                | ResponseItem::WebSearchCall { .. }
                | ResponseItem::Other
                | ResponseItem::Compaction { .. } => {
                    continue;
                }
            }
        }

        // Sanitize messages for strict chat completions providers:
        // 1. Merge adjacent assistant text + assistant tool_call messages
        //    into a single message (some providers reject split messages).
        // 2. Remove non-standard tool call types (local_shell_call, custom)
        //    and their orphaned tool results.
        if !provider.is_openai() {
            messages = sanitize_chat_messages(messages);
        }

        // Debug: dump the messages array for non-OpenAI providers.
        if !provider.is_openai() {
            warn!(
                "[chat-sanitize] total messages after sanitize: {}",
                messages.len()
            );
            for (i, msg) in messages.iter().enumerate() {
                let role = msg.get("role").and_then(Value::as_str).unwrap_or("?");
                let tc_count = msg
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .map(|a| a.len());
                let tc_id = msg.get("tool_call_id").and_then(Value::as_str);
                let content_preview = msg
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| {
                        let truncated: String = s.chars().take(80).collect();
                        if truncated.len() < s.len() {
                            format!("{truncated}...")
                        } else {
                            truncated
                        }
                    })
                    .unwrap_or_else(|| "N/A".to_string());
                warn!(
                    "[chat-msg {i}] role={role} tool_calls={tc_count:?} tool_call_id={tc_id:?} content={content_preview}"
                );
            }
        }

        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "stream_options": {
                "include_usage": true
            },
            "tools": self.tools,
        });

        // Add thinking parameter for Volcengine provider
        if provider.is_volcengine() {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("thinking".to_string(), json!({"type": "enabled"}));
            }
        }

        let mut headers = build_conversation_headers(self.conversation_id);
        if let Some(subagent) = subagent_header(&self.session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        Ok(ChatRequest {
            body: payload,
            headers,
        })
    }
}

fn push_tool_call_message(messages: &mut Vec<Value>, tool_call: Value, reasoning: Option<&str>) {
    // Chat Completions requires that tool calls are grouped into a single assistant message
    // (with `tool_calls: [...]`) followed by tool role responses.
    if let Some(Value::Object(obj)) = messages.last_mut()
        && obj.get("role").and_then(Value::as_str) == Some("assistant")
        && obj.get("content").is_some_and(Value::is_null)
        && let Some(tool_calls) = obj.get_mut("tool_calls").and_then(Value::as_array_mut)
    {
        tool_calls.push(tool_call);
        if let Some(reasoning) = reasoning {
            if let Some(Value::String(existing)) = obj.get_mut("reasoning") {
                if !existing.is_empty() {
                    existing.push('\n');
                }
                existing.push_str(reasoning);
            } else {
                obj.insert(
                    "reasoning".to_string(),
                    Value::String(reasoning.to_string()),
                );
            }
        }
        return;
    }

    let mut msg = json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [tool_call],
    });
    if let Some(reasoning) = reasoning
        && let Some(obj) = msg.as_object_mut()
    {
        obj.insert("reasoning".to_string(), json!(reasoning));
    }
    messages.push(msg);
}

/// Sanitize the chat messages array for strict chat completions providers.
///
/// 1. Merge consecutive assistant messages: if an assistant message with text
///    content is immediately followed by another assistant message carrying
///    `tool_calls`, merge the `tool_calls` into the first message so that the
///    provider sees a single assistant turn.
///
/// 2. Collect the set of valid tool-call IDs from assistant messages and drop
///    any `tool` role messages whose `tool_call_id` doesn't appear in that set
///    (orphaned results from non-standard call types like `local_shell_call`).
///
/// 3. Strip non-standard entries from `tool_calls` arrays (keeping only
///    `"type": "function"`) and remove assistant messages that end up with an
///    empty `tool_calls` array after filtering.
fn sanitize_chat_messages(messages: Vec<Value>) -> Vec<Value> {
    use std::collections::HashSet;

    // --- Pass 1: merge ALL adjacent assistant messages into one.
    // MiniMax (and other strict providers) require that tool results immediately
    // follow the assistant message containing tool_calls.  The codex conversation
    // model can produce separate assistant messages for text, reasoning, and
    // tool_calls which violates that constraint.
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for msg in messages {
        let dominated = if let Some(Value::Object(prev)) = merged.last_mut()
            && prev.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(cur) = msg.as_object()
            && cur.get("role").and_then(Value::as_str) == Some("assistant")
        {
            // Merge tool_calls arrays
            if let Some(Value::Array(incoming)) = cur.get("tool_calls") {
                if let Some(Value::Array(existing)) = prev.get_mut("tool_calls") {
                    existing.extend(incoming.iter().cloned());
                } else {
                    prev.insert("tool_calls".to_string(), Value::Array(incoming.clone()));
                }
            }

            // Merge content: if cur has non-null content, set it on prev
            // (prefer the first non-null content encountered)
            let prev_has_content = prev
                .get("content")
                .is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
            if !prev_has_content {
                if let Some(c) = cur.get("content")
                    && !c.is_null()
                    && c.as_str() != Some("")
                {
                    prev.insert("content".to_string(), c.clone());
                }
            }

            true
        } else {
            false
        };

        if !dominated {
            merged.push(msg);
        }
    }

    // --- Pass 2: filter non-standard tool call types and collect valid IDs
    let mut valid_tool_call_ids: HashSet<String> = HashSet::new();
    let mut cleaned: Vec<Value> = Vec::with_capacity(merged.len());

    for mut msg in merged {
        let dominated = if let Some(obj) = msg.as_object_mut()
            && obj.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(Value::Array(tool_calls)) = obj.get_mut("tool_calls")
        {
            // Keep only "function" type tool calls
            tool_calls.retain(|tc| {
                let is_function = tc
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t == "function");
                if is_function {
                    if let Some(id) = tc.get("id").and_then(Value::as_str) {
                        valid_tool_call_ids.insert(id.to_string());
                    }
                }
                is_function
            });
            // If all tool calls were removed, drop the tool_calls key.
            // If the message also has no meaningful content, skip it entirely.
            if tool_calls.is_empty() {
                obj.remove("tool_calls");
                let has_content = obj
                    .get("content")
                    .is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
                !has_content
            } else {
                false
            }
        } else {
            false
        };

        if !dominated {
            cleaned.push(msg);
        }
    }

    // --- Pass 3: drop orphaned tool results
    cleaned.retain(|msg| {
        if let Some(obj) = msg.as_object()
            && obj.get("role").and_then(Value::as_str) == Some("tool")
        {
            obj.get("tool_call_id")
                .and_then(Value::as_str)
                .is_some_and(|id| valid_tool_call_ids.contains(id))
        } else {
            true
        }
    });

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::RetryConfig;
    use crate::provider::WireApi;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::SubAgentSource;
    use http::HeaderValue;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    fn provider() -> Provider {
        Provider {
            name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            query_params: None,
            wire: WireApi::Chat,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(10),
                retry_429: false,
                retry_5xx: true,
                retry_transport: true,
            },
            stream_idle_timeout: Duration::from_secs(1),
            system_role: None,
        }
    }

    #[test]
    fn attaches_conversation_and_subagent_headers() {
        let prompt_input = vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "hi".to_string(),
            }],
            end_turn: None,
        }];
        let req = ChatRequestBuilder::new("gpt-test", "inst", &prompt_input, &[])
            .conversation_id(Some("conv-1".into()))
            .session_source(Some(SessionSource::SubAgent(SubAgentSource::Review)))
            .build(&provider())
            .expect("request");

        assert_eq!(
            req.headers.get("session_id"),
            Some(&HeaderValue::from_static("conv-1"))
        );
        assert_eq!(
            req.headers.get("x-openai-subagent"),
            Some(&HeaderValue::from_static("review"))
        );
    }

    #[test]
    fn groups_consecutive_tool_calls_into_a_single_assistant_message() {
        let prompt_input = vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "read these".to_string(),
                }],
                end_turn: None,
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "read_file".to_string(),
                arguments: r#"{"path":"a.txt"}"#.to_string(),
                call_id: "call-a".to_string(),
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "read_file".to_string(),
                arguments: r#"{"path":"b.txt"}"#.to_string(),
                call_id: "call-b".to_string(),
            },
            ResponseItem::FunctionCall {
                id: None,
                name: "read_file".to_string(),
                arguments: r#"{"path":"c.txt"}"#.to_string(),
                call_id: "call-c".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-a".to_string(),
                output: FunctionCallOutputPayload {
                    content: "A".to_string(),
                    ..Default::default()
                },
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-b".to_string(),
                output: FunctionCallOutputPayload {
                    content: "B".to_string(),
                    ..Default::default()
                },
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call-c".to_string(),
                output: FunctionCallOutputPayload {
                    content: "C".to_string(),
                    ..Default::default()
                },
            },
        ];

        let req = ChatRequestBuilder::new("gpt-test", "inst", &prompt_input, &[])
            .build(&provider())
            .expect("request");

        let messages = req
            .body
            .get("messages")
            .and_then(|v| v.as_array())
            .expect("messages array");
        // system + user + assistant(tool_calls=[...]) + 3 tool outputs
        assert_eq!(messages.len(), 6);

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");

        let tool_calls_msg = &messages[2];
        assert_eq!(tool_calls_msg["role"], "assistant");
        assert_eq!(tool_calls_msg["content"], serde_json::Value::Null);
        let tool_calls = tool_calls_msg["tool_calls"]
            .as_array()
            .expect("tool_calls array");
        assert_eq!(tool_calls.len(), 3);
        assert_eq!(tool_calls[0]["id"], "call-a");
        assert_eq!(tool_calls[1]["id"], "call-b");
        assert_eq!(tool_calls[2]["id"], "call-c");

        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call-a");
        assert_eq!(messages[4]["role"], "tool");
        assert_eq!(messages[4]["tool_call_id"], "call-b");
        assert_eq!(messages[5]["role"], "tool");
        assert_eq!(messages[5]["tool_call_id"], "call-c");
    }
}
