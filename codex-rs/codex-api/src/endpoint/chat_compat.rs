use crate::auth::AuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::chat_compat::ChatRequestBuilder;
use crate::sse::chat_compat::ChatReasoningFormat;
use crate::sse::chat_compat::spawn_chat_stream;
use crate::telemetry::SseTelemetry;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::RequestTelemetry;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use http::HeaderValue;
use http::Method;
use serde_json::Value;
use std::sync::Arc;

pub struct ChatCompatClient<T: HttpTransport, A: AuthProvider> {
    session: EndpointSession<T, A>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport, A: AuthProvider> ChatCompatClient<T, A> {
    pub fn new(transport: T, provider: Provider, auth: A) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    pub async fn stream_prompt(
        &self,
        model: &str,
        instructions: &str,
        input: &[ResponseItem],
        tools: &[Value],
        conversation_id: Option<String>,
        session_source: Option<SessionSource>,
    ) -> Result<ResponseStream, ApiError> {
        let provider = self.session.provider();
        let reasoning_format = chat_reasoning_format(provider);
        let mut request = ChatRequestBuilder::new(model, instructions, input, tools)
            .conversation_id(conversation_id)
            .session_source(session_source)
            .build(provider)?;

        merge_split_assistant_messages(&mut request.body);

        // Fork: Zhipu requires thinking + tool_stream parameters.
        if provider.is_zhipu() {
            inject_zhipu_params(&mut request.body);
        }

        let stream_response = self
            .session
            .stream_with(
                Method::POST,
                "chat/completions",
                request.headers,
                Some(request.body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = RequestCompression::None;
                },
            )
            .await?;

        Ok(spawn_chat_stream(
            stream_response,
            provider.stream_idle_timeout,
            self.sse_telemetry.clone(),
            reasoning_format,
            None,
        ))
    }
}

fn chat_reasoning_format(_provider: &Provider) -> ChatReasoningFormat {
    // Keep think tags in assistant content rather than extracting them into
    // separate Reasoning items.  MiniMax (and potentially other Chat
    // Completions providers) relies on seeing its own `<think>` tags inline;
    // extracting them corrupts the conversation history round-trip because
    // reasoning attached to tool-call messages (where content is null) is
    // lost entirely.
    ChatReasoningFormat::Standard
}

/// Fork: Inject Zhipu-specific parameters into the Chat Completions request body.
///
/// Zhipu requires `thinking: {"type": "enabled"}` to activate reasoning and
/// `tool_stream: true` to enable streaming of tool call arguments.
fn inject_zhipu_params(body: &mut serde_json::Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("thinking".into(), serde_json::json!({"type": "enabled"}));
        obj.insert("tool_stream".into(), serde_json::json!(true));
    }
}

/// Merge consecutive assistant messages where the first has content and the
/// second has `tool_calls`.
///
/// The SSE parser emits assistant content and tool calls as separate
/// `ResponseItem`s.  The request builder serialises them as two consecutive
/// `role: "assistant"` messages — one with content, one with `tool_calls`.
/// Standard Chat Completions format expects a single message carrying both.
/// Sending two confuses some providers (MiniMax in particular) into generating
/// text-only replies on subsequent turns.
fn merge_split_assistant_messages(body: &mut serde_json::Value) {
    let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };

    let mut i = 0;
    while i + 1 < messages.len() {
        let should_merge = {
            let (a, b) = (&messages[i], &messages[i + 1]);
            let a_obj = a.as_object();
            let b_obj = b.as_object();
            match (a_obj, b_obj) {
                (Some(a), Some(b)) => {
                    a.get("role").and_then(|r| r.as_str()) == Some("assistant")
                        && b.get("role").and_then(|r| r.as_str()) == Some("assistant")
                        && a.get("content").is_some_and(|c| !c.is_null())
                        && !a.contains_key("tool_calls")
                        && b.contains_key("tool_calls")
                        && b.get("content").is_none_or(serde_json::Value::is_null)
                }
                _ => false,
            }
        };

        if should_merge {
            let first = messages.remove(i);
            let Some(second) = messages[i].as_object_mut() else {
                continue;
            };

            if let Some(content) = first.get("content") {
                second.insert("content".to_string(), content.clone());
            }
            if let Some(reasoning) = first.get("reasoning") {
                second.insert("reasoning".to_string(), reasoning.clone());
            }
            // Don't increment — recheck at the same index.
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn merges_split_assistant_content_and_tool_calls() {
        let mut body = json!({
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Do something"},
                {"role": "assistant", "content": "<think>plan</think>I'll do it"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "shell", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "done"}
            ]
        });

        merge_split_assistant_messages(&mut body);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "<think>plan</think>I'll do it");
        assert!(messages[2]["tool_calls"].is_array());
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn preserves_reasoning_field_during_merge() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": "text", "reasoning": "thought"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
                ]}
            ]
        });

        merge_split_assistant_messages(&mut body);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "text");
        assert_eq!(messages[0]["reasoning"], "thought");
        assert!(messages[0]["tool_calls"].is_array());
    }

    #[test]
    fn does_not_merge_when_already_combined() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": "hello", "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
                ]}
            ]
        });

        merge_split_assistant_messages(&mut body);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn does_not_merge_non_consecutive_roles() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": "hello"},
                {"role": "user", "content": "world"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
                ]}
            ]
        });

        merge_split_assistant_messages(&mut body);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn handles_multiple_split_pairs() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": "first"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f1", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "r1"},
                {"role": "assistant", "content": "second"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c2", "type": "function", "function": {"name": "f2", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "c2", "content": "r2"}
            ]
        });

        merge_split_assistant_messages(&mut body);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["content"], "first");
        assert!(messages[0]["tool_calls"].is_array());
        assert_eq!(messages[2]["content"], "second");
        assert!(messages[2]["tool_calls"].is_array());
    }
}
