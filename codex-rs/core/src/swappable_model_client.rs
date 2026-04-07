/// Fork: thin wrapper around [`ModelClient`] that supports mid-session replacement
/// for `Op::OverrideProvider` while keeping call sites close to upstream.
///
/// Uses `std::sync::RwLock` internally. Read-side async methods clone the inner client
/// before awaiting so the lock is never held across an `.await` point.
use std::sync::Arc;
use std::sync::RwLock;
use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use codex_api::MemorySummarizeOutput as ApiMemorySummarizeOutput;
use codex_api::RawMemory as ApiRawMemory;
use codex_login::AuthManager;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_rollout_trace::CompactionTraceContext;

pub(crate) struct SwappableModelClient {
    inner: RwLock<ModelClient>,
}

// RwLock poisoning means a prior panic — unrecoverable, so expect is appropriate.
#[allow(clippy::expect_used)]
impl SwappableModelClient {
    pub(crate) fn new(client: ModelClient) -> Self {
        Self {
            inner: RwLock::new(client),
        }
    }

    pub(crate) fn new_session(&self) -> ModelClientSession {
        self.inner.read().expect("lock poisoned").new_session()
    }

    /// Fork: clone the inner `ModelClient` (cheap `Arc` bump) for passing to
    /// `RegularTask::with_startup_prewarm()` which expects a `ModelClient`.
    pub(crate) fn clone_inner(&self) -> ModelClient {
        self.inner.read().expect("lock poisoned").clone()
    }

    pub(crate) fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.inner.read().expect("lock poisoned").auth_manager()
    }

    pub(crate) fn set_window_generation(&self, window_generation: u64) {
        self.inner
            .read()
            .expect("lock poisoned")
            .set_window_generation(window_generation);
    }

    pub(crate) fn advance_window_generation(&self) {
        self.inner
            .read()
            .expect("lock poisoned")
            .advance_window_generation();
    }

    pub(crate) async fn compact_conversation_history(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        session_telemetry: &SessionTelemetry,
        compaction_trace: &CompactionTraceContext,
    ) -> Result<Vec<ResponseItem>> {
        let client = self.inner.read().expect("lock poisoned").clone();
        client
            .compact_conversation_history(
                prompt,
                model_info,
                effort,
                summary,
                session_telemetry,
                compaction_trace,
            )
            .await
    }

    /// Async: memory summarization. Same clone-then-release pattern.
    /// Not currently called via `services.model_client` but included for
    /// forward compatibility if upstream routes it through the wrapper.
    #[allow(dead_code)]
    pub(crate) async fn summarize_memories(
        &self,
        raw_memories: Vec<ApiRawMemory>,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
        session_telemetry: &SessionTelemetry,
    ) -> Result<Vec<ApiMemorySummarizeOutput>> {
        let client = self.inner.read().expect("lock poisoned").clone();
        client
            .summarize_memories(raw_memories, model_info, effort, session_telemetry)
            .await
    }

    pub(crate) fn responses_websocket_enabled(&self) -> bool {
        self.inner
            .read()
            .expect("lock poisoned")
            .responses_websocket_enabled()
    }

    pub(crate) fn replace(&self, new_client: ModelClient) {
        *self.inner.write().expect("lock poisoned") = new_client;
    }
}
