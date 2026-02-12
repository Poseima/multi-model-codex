/// Fork: thin wrapper around [`ModelClient`] that supports mid-session replacement
/// for `Op::OverrideProvider` while presenting the same method signatures to callers.
///
/// Uses `std::sync::RwLock` internally. All read-side methods are either sync
/// (`new_session`) or clone-then-release (`compact_conversation_history`,
/// `summarize_memories`), so the lock is never held across an `.await` point.
use std::sync::RwLock;

use codex_api::MemorySummarizeOutput as ApiMemorySummarizeOutput;
use codex_api::RawMemory as ApiRawMemory;
use codex_otel::OtelManager;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;

use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::error::Result;

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

    /// Sync: creates a turn-scoped session.
    pub(crate) fn new_session(&self) -> ModelClientSession {
        self.inner.read().expect("lock poisoned").new_session()
    }

    /// Fork: clone the inner `ModelClient` (cheap `Arc` bump) for passing to
    /// `RegularTask::with_startup_prewarm()` which expects a `ModelClient`.
    pub(crate) fn clone_inner(&self) -> ModelClient {
        self.inner.read().expect("lock poisoned").clone()
    }

    /// Async: remote compaction. Clones the inner client (cheap `Arc` bump),
    /// releases the lock, then awaits the call.
    pub(crate) async fn compact_conversation_history(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        otel_manager: &OtelManager,
    ) -> Result<Vec<ResponseItem>> {
        let client = self.inner.read().expect("lock poisoned").clone();
        client
            .compact_conversation_history(prompt, model_info, otel_manager)
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
        otel_manager: &OtelManager,
    ) -> Result<Vec<ApiMemorySummarizeOutput>> {
        let client = self.inner.read().expect("lock poisoned").clone();
        client
            .summarize_memories(raw_memories, model_info, effort, otel_manager)
            .await
    }

    /// Sync: delegates to inner client's `responses_websocket_enabled`.
    pub(crate) fn responses_websocket_enabled(&self, model_info: &ModelInfo) -> bool {
        self.inner
            .read()
            .expect("lock poisoned")
            .responses_websocket_enabled(model_info)
    }

    /// Fork: replace the inner client when the user switches providers.
    pub(crate) fn replace(&self, new_client: ModelClient) {
        *self.inner.write().expect("lock poisoned") = new_client;
    }
}
