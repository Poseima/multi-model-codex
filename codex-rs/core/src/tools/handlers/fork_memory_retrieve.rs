//! Fork: memory retrieval tool handler.
//!
//! Provides the `memory_retrieve` tool that lets the main agent load project
//! memory files on demand, guided by memory clues in the system prompt.

use async_trait::async_trait;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MemoryRetrieveBeginEvent;
use codex_protocol::protocol::MemoryRetrieveEndEvent;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::client_common::tools::ResponsesApiTool;
use crate::client_common::tools::ToolSpec;
use crate::function_tool::FunctionCallError;
use crate::memory_experiment;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::spec::JsonSchema;

pub struct MemoryRetrieveHandler;

/// Build the JSON schema tool spec for `memory_retrieve`.
pub fn tool_spec() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "query".to_string(),
        JsonSchema::String {
            description: Some(
                "Detailed description of what context you need from project memories".to_string(),
            ),
        },
    );

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_retrieve".to_string(),
        description: "Research project memories and return synthesized findings for your query. \
            Use when memory clues in the system prompt match your task."
            .to_string(),
        strict: false,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["query".to_string()]),
            additional_properties: Some(false.into()),
        },
    })
}

#[derive(Deserialize)]
struct MemoryRetrieveArgs {
    #[serde(default)]
    query: String,
}

#[async_trait]
impl ToolHandler for MemoryRetrieveHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_retrieve handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: MemoryRetrieveArgs = parse_arguments(&arguments)?;

        // Check if experiment is active.
        if !memory_experiment::is_enabled(&turn.config.codex_home, &turn.cwd, &turn.features) {
            return Ok(ToolOutput::Function {
                body: FunctionCallOutputBody::Text(
                    "Memory experiment is not enabled for this project. \
                     No memories available."
                        .to_string(),
                ),
                success: Some(false),
            });
        }

        let project_root =
            memory_experiment::get_project_memory_root(&turn.config.codex_home, &turn.cwd);

        // Emit begin event so TUI can show "Retrieving memories".
        session
            .send_event(
                &turn,
                EventMsg::MemoryRetrieveBegin(MemoryRetrieveBeginEvent {
                    query: args.query.clone(),
                }),
            )
            .await;

        let result =
            memory_experiment::retrieval::retrieve(&project_root, &args.query, &session, &turn)
                .await;

        let success = result.is_ok();

        // Emit end event so TUI can show "Memory retrieved".
        session
            .send_event(
                &turn,
                EventMsg::MemoryRetrieveEnd(MemoryRetrieveEndEvent {
                    query: args.query.clone(),
                    success,
                }),
            )
            .await;

        let content = result.map_err(FunctionCallError::RespondToModel)?;
        let output = format!("<memory triggered>\n{content}\n</memory triggered>");

        Ok(ToolOutput::Function {
            body: FunctionCallOutputBody::Text(output),
            success: Some(true),
        })
    }
}
