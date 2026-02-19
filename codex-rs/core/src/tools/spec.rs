use crate::shell::Shell;
use crate::shell::ShellType;
use crate::tools::handlers::multi_agents_common::DEFAULT_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MAX_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MIN_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_spec::WaitAgentTimeoutOptions;
use crate::tools::registry::ToolRegistryBuilder;
use crate::tools::spec_plan::build_tool_registry_builder;
use crate::tools::spec_plan_types::ToolRegistryBuildParams;
use codex_mcp::ToolInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_tool_api::ToolBundle as ExtensionToolBundle;
use codex_tools::DiscoverableTool;
use codex_tools::ToolUserShellType;
use codex_tools::ToolsConfig;
use codex_tools::create_tools_json_for_responses_api;
use serde_json::json;

pub(crate) fn tool_user_shell_type(user_shell: &Shell) -> ToolUserShellType {
    match user_shell.shell_type {
        ShellType::Zsh => ToolUserShellType::Zsh,
        ShellType::Bash => ToolUserShellType::Bash,
        ShellType::PowerShell => ToolUserShellType::PowerShell,
        ShellType::Sh => ToolUserShellType::Sh,
        ShellType::Cmd => ToolUserShellType::Cmd,
    }
}

/// Converts Responses API tool definitions into the Chat Completions API
/// wrapper shape and drops non-function tools.
pub(crate) fn create_tools_json_for_chat_completions_api(
    tools: &[codex_tools::ToolSpec],
) -> codex_protocol::error::Result<Vec<serde_json::Value>> {
    let responses_api_tools_json = create_tools_json_for_responses_api(tools)?;
    Ok(responses_api_tools_json
        .into_iter()
        .filter_map(|mut tool| {
            if tool.get("type") != Some(&serde_json::Value::String("function".to_string())) {
                return None;
            }
            let map = tool.as_object_mut()?;
            let name = map
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            map.remove("type");
            Some(json!({ "type": "function", "name": name, "function": map }))
        })
        .collect())
}

pub(crate) fn build_specs_with_discoverable_tools(
    config: &ToolsConfig,
    mcp_tools: Option<Vec<ToolInfo>>,
    deferred_mcp_tools: Option<Vec<ToolInfo>>,
    discoverable_tools: Option<Vec<DiscoverableTool>>,
    extension_tool_bundles: &[ExtensionToolBundle],
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    let default_agent_type_description =
        crate::agent::role::spawn_tool_spec::build(&std::collections::BTreeMap::new());
    let min_wait_timeout_ms = if config.multi_agent_v2 {
        config
            .wait_agent_min_timeout_ms
            .unwrap_or(MIN_WAIT_TIMEOUT_MS)
            .clamp(1, MAX_WAIT_TIMEOUT_MS)
    } else {
        MIN_WAIT_TIMEOUT_MS
    };
    let default_wait_timeout_ms =
        DEFAULT_WAIT_TIMEOUT_MS.clamp(min_wait_timeout_ms, MAX_WAIT_TIMEOUT_MS);
    build_tool_registry_builder(
        config,
        ToolRegistryBuildParams {
            mcp_tools: mcp_tools.as_deref(),
            deferred_mcp_tools: deferred_mcp_tools.as_deref(),
            discoverable_tools: discoverable_tools.as_deref(),
            extension_tool_bundles,
            dynamic_tools,
            default_agent_type_description: &default_agent_type_description,
            wait_agent_timeouts: WaitAgentTimeoutOptions {
                default_timeout_ms: default_wait_timeout_ms,
                min_timeout_ms: min_wait_timeout_ms,
                max_timeout_ms: MAX_WAIT_TIMEOUT_MS,
            },
        },
    )
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
