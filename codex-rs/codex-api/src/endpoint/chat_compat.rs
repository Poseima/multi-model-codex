use crate::auth::AuthProvider;
use crate::common::Prompt as ApiPrompt;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::chat_compat::ChatRequestBuilder;
use crate::sse::chat_compat::spawn_chat_stream;
use crate::telemetry::SseTelemetry;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::RequestTelemetry;
use codex_protocol::protocol::SessionSource;
use http::HeaderValue;
use http::Method;
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
        prompt: &ApiPrompt,
        conversation_id: Option<String>,
        session_source: Option<SessionSource>,
    ) -> Result<ResponseStream, ApiError> {
        let request =
            ChatRequestBuilder::new(model, &prompt.instructions, &prompt.input, &prompt.tools)
                .conversation_id(conversation_id)
                .session_source(session_source)
                .build(self.session.provider())?;

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
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            None,
        ))
    }
}
