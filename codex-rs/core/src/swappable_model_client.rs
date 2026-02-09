/// Fork: thin wrapper around [`ModelClient`] that supports mid-session replacement
/// for `Op::OverrideProvider` while keeping call sites close to upstream.
///
/// Uses `std::sync::RwLock` internally. Read-side async methods clone the inner client
/// before awaiting so the lock is never held across an `.await` point.
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;

use codex_api::MemorySummarizeOutput as ApiMemorySummarizeOutput;
use codex_api::RawMemory as ApiRawMemory;
use codex_otel::SessionTelemetry;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_rollout_trace::CompactionTraceContext;

use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::error::Result;
use crate::responses_metadata::CodexResponsesMetadata;

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

    pub(crate) fn clone_client(&self) -> ModelClient {
        self.inner.read().unwrap().clone()
    }

    pub(crate) async fn compact_conversation_history(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        turn_state: Option<Arc<OnceLock<String>>>,
        settings: CompactConversationRequestSettings,
        session_telemetry: &SessionTelemetry,
        compaction_trace: &CompactionTraceContext,
        responses_metadata: &CodexResponsesMetadata,
    ) -> Result<Vec<ResponseItem>> {
        let client = self.inner.read().expect("lock poisoned").clone();
        client
            .compact_conversation_history(
                prompt,
                model_info,
                turn_state,
                settings,
                session_telemetry,
                compaction_trace,
                responses_metadata,
            )
            .await
    }

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
        self.inner.read().expect("lock poisoned").responses_websocket_enabled()
    }

    pub(crate) fn replace(&self, new_client: ModelClient) {
        *self.inner.write().expect("lock poisoned") = new_client;
    }
}
