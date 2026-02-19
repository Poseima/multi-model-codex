pub(crate) mod control;
pub(crate) mod fork_memory_role; // Fork: memory retriever role enrichment
mod guards;
pub(crate) mod role;
pub(crate) mod status;

pub(crate) use codex_protocol::protocol::AgentStatus;
pub(crate) use control::AgentControl;
pub(crate) use guards::exceeds_thread_spawn_depth_limit;
pub(crate) use guards::next_thread_spawn_depth;
pub(crate) use status::agent_status_from_event;
