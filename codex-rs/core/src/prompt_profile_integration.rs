use codex_api::Provider;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::prompt_profile::PromptSource;
use codex_protocol::protocol::InitialHistory;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptProfileSelection {
    InheritFromHistory,
    Set(Box<PromptSource>),
    Clear,
}

impl PromptProfileSelection {
    pub(crate) fn for_new_thread(prompt_profile: Option<PromptSource>) -> Self {
        match prompt_profile {
            Some(prompt_profile) => Self::Set(Box::new(prompt_profile)),
            None => Self::Clear,
        }
    }

    pub(crate) fn for_fork_override(prompt_profile: Option<PromptSource>) -> Self {
        match prompt_profile {
            Some(prompt_profile) => Self::Set(Box::new(prompt_profile)),
            None => Self::InheritFromHistory,
        }
    }

    pub(crate) fn resolve(&self, initial_history: &InitialHistory) -> Option<PromptSource> {
        match self {
            Self::InheritFromHistory => initial_history.get_prompt_profile(),
            Self::Set(prompt_profile) => Some((**prompt_profile).clone()),
            Self::Clear => None,
        }
    }
}

pub(crate) fn compose_base_instructions(
    base_instructions: &str,
    prompt_profile: Option<&PromptSource>,
) -> String {
    crate::prompt_profile_render::compose_base_instructions(base_instructions, prompt_profile)
}

pub(crate) fn format_instructions(
    base_instructions: &str,
    prompt_profile: Option<&PromptSource>,
    input: &[ResponseItem],
    render_options: crate::prompt_profile_render::PromptProfileRenderOptions,
) -> String {
    crate::prompt_profile_render::format_instructions(
        base_instructions,
        prompt_profile,
        input,
        render_options,
    )
}

pub(crate) fn format_input(
    input: &[ResponseItem],
    prompt_profile: Option<&PromptSource>,
) -> Vec<ResponseItem> {
    crate::prompt_profile_render::format_input(input, prompt_profile)
}

pub(crate) fn format_input_for_chat_provider(
    input: &[ResponseItem],
    prompt_profile: Option<&PromptSource>,
    provider: &Provider,
) -> Vec<ResponseItem> {
    let formatted = crate::prompt_profile_render::format_input(input, prompt_profile);
    let Some(prompt_profile) = prompt_profile else {
        return formatted;
    };

    if provider.effective_system_role() != "system" {
        return formatted;
    }

    let Some(bridge) =
        crate::prompt_profile_render::build_chat_tool_continuation_bridge(input, prompt_profile)
    else {
        return formatted;
    };

    rewrite_chat_tool_continuation_tail(formatted, bridge)
}

pub(crate) fn build_primary_greeting_item(
    prompt_profile: Option<&PromptSource>,
) -> Option<ResponseItem> {
    crate::prompt_profile_render::build_primary_greeting_item(prompt_profile)
}

fn rewrite_chat_tool_continuation_tail(
    mut formatted: Vec<ResponseItem>,
    bridge: crate::prompt_profile_render::ChatToolContinuationBridge,
) -> Vec<ResponseItem> {
    let reminder_index = promote_matching_message_to_system(
        &mut formatted,
        "developer",
        bridge.post_tool_roleplay.as_str(),
    );
    let prefix_index = bridge
        .assistant_prefix
        .as_deref()
        .and_then(|prefix| find_matching_message(&formatted, "assistant", prefix));

    let reminder_index = reminder_index.unwrap_or_else(|| {
        let insertion_index = prefix_index.unwrap_or(formatted.len());
        formatted.insert(
            insertion_index,
            render_message("system", bridge.post_tool_roleplay.clone()),
        );
        insertion_index
    });

    let scaffold_index = prefix_index.unwrap_or(reminder_index + 1);
    formatted.insert(
        scaffold_index,
        render_message("system", bridge.continuation_scaffold),
    );

    formatted
}

fn promote_matching_message_to_system(
    formatted: &mut [ResponseItem],
    expected_role: &str,
    expected_text: &str,
) -> Option<usize> {
    let index = find_matching_message(formatted, expected_role, expected_text)?;
    let ResponseItem::Message {
        id,
        content,
        end_turn,
        phase,
        ..
    } = &formatted[index]
    else {
        return None;
    };

    formatted[index] = ResponseItem::Message {
        id: id.clone(),
        role: "system".to_string(),
        content: content.clone(),
        end_turn: *end_turn,
        phase: phase.clone(),
    };
    Some(index)
}

fn find_matching_message(
    formatted: &[ResponseItem],
    expected_role: &str,
    expected_text: &str,
) -> Option<usize> {
    formatted.iter().rposition(|item| {
        if let ResponseItem::Message { role, content, .. } = item {
            role == expected_role && message_text(content) == expected_text
        } else {
            false
        }
    })
}

fn message_text(content: &[ContentItem]) -> String {
    content
        .iter()
        .map(|content| match content {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => text.clone(),
            ContentItem::InputImage { image_url } => image_url.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_message(role: &str, text: String) -> ResponseItem {
    let content = match role {
        "assistant" => vec![ContentItem::OutputText { text }],
        "system" | "developer" | "user" => vec![ContentItem::InputText { text }],
        _ => unreachable!("unexpected role"),
    };
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        end_turn: None,
        phase: None,
    }
}

#[cfg(test)]
mod tests {
    use super::PromptProfileSelection;
    use super::format_input_for_chat_provider;
    use crate::prompt_profile_import::load_prompt_profile_from_path;
    use codex_api::Provider;
    use codex_api::provider::RetryConfig;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::prompt_profile::PromptInjectionRole;
    use codex_protocol::prompt_profile::PromptKnowledgeEntry;
    use codex_protocol::prompt_profile::PromptKnowledgeSource;
    use codex_protocol::prompt_profile::PromptSource;
    use codex_protocol::prompt_profile::PromptSourceOrigin;
    use codex_protocol::protocol::InitialHistory;
    use codex_protocol::protocol::RolloutItem;
    use codex_protocol::protocol::SessionMeta;
    use codex_protocol::protocol::SessionMetaLine;
    use codex_utils_cargo_bin::find_resource;
    use http::HeaderMap;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::time::Duration;

    fn prompt_profile(name: &str) -> PromptSource {
        PromptSource {
            name: Some(name.to_string()),
            ..Default::default()
        }
    }

    fn zhipu_chat_provider() -> Provider {
        Provider {
            name: "Zhipu".to_string(),
            base_url: "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: false,
                retry_transport: false,
            },
            stream_idle_timeout: Duration::from_secs(30),
            system_role: None,
        }
    }

    fn minimax_chat_provider() -> Provider {
        Provider {
            system_role: Some("user".to_string()),
            ..zhipu_chat_provider()
        }
    }

    fn bundled_xie_zhilin_prompt_profile() -> PromptSource {
        let path = find_resource!("tests/fixtures/xie_zhilin_card_v3.png")
            .expect("resolve bundled Xie Zhiling PNG fixture");
        load_prompt_profile_from_path(&path).expect("load prompt profile")
    }

    fn render_message(role: PromptInjectionRole, text: &str) -> ResponseItem {
        let role = match role {
            PromptInjectionRole::System => "system",
            PromptInjectionRole::Developer => "developer",
            PromptInjectionRole::User => "user",
            PromptInjectionRole::Assistant => "assistant",
        };
        let content = match role {
            "assistant" => vec![codex_protocol::models::ContentItem::OutputText {
                text: text.to_string(),
            }],
            "system" | "developer" | "user" => {
                vec![codex_protocol::models::ContentItem::InputText {
                    text: text.to_string(),
                }]
            }
            _ => unreachable!("unexpected role"),
        };
        ResponseItem::Message {
            id: None,
            role: role.to_string(),
            content,
            end_turn: None,
            phase: None,
        }
    }

    fn fork_history_with_prompt_profile(prompt_profile: PromptSource) -> InitialHistory {
        InitialHistory::Forked(vec![RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                prompt_profile: Some(prompt_profile),
                ..Default::default()
            },
            git: None,
        })])
    }

    #[test]
    fn prompt_profile_selection_for_new_thread_clears_when_unset() {
        let history = fork_history_with_prompt_profile(prompt_profile("history"));

        let resolved = PromptProfileSelection::for_new_thread(None).resolve(&history);

        assert_eq!(resolved, None);
    }

    #[test]
    fn prompt_profile_selection_for_fork_inherits_when_unset() {
        let history_profile = prompt_profile("history");
        let history = fork_history_with_prompt_profile(history_profile.clone());

        let resolved = PromptProfileSelection::for_fork_override(None).resolve(&history);

        assert_eq!(resolved, Some(history_profile));
    }

    #[test]
    fn prompt_profile_selection_for_fork_prefers_explicit_override() {
        let history = fork_history_with_prompt_profile(prompt_profile("history"));
        let explicit = prompt_profile("explicit");

        let resolved =
            PromptProfileSelection::for_fork_override(Some(explicit.clone())).resolve(&history);

        assert_eq!(resolved, Some(explicit));
    }

    #[test]
    fn prompt_profile_selection_clear_drops_history_profile() {
        let history = fork_history_with_prompt_profile(prompt_profile("history"));

        let resolved = PromptProfileSelection::Clear.resolve(&history);

        assert_eq!(resolved, None);
    }

    #[test]
    fn chat_provider_tool_continuation_promotes_roleplay_tail_to_system_scaffold() {
        let prompt_profile = PromptSource {
            name: Some("谢知凛".to_string()),
            origin: Some(PromptSourceOrigin {
                format: Some("sillytavern-v3".to_string()),
                ..Default::default()
            }),
            variables: [("user_name".to_string(), "林夏".to_string())]
                .into_iter()
                .collect(),
            knowledge: vec![PromptKnowledgeSource {
                name: Some("World".to_string()),
                kind: Some("worldBook".to_string()),
                description: None,
                entries: vec![PromptKnowledgeEntry {
                    id: Some("assistant-prefix".to_string()),
                    keys: Vec::new(),
                    secondary_keys: Vec::new(),
                    content: "<system_constraints>\n...\n<status_bar>\n[state|tense]\n</status_bar>\n<system_bar>\n[任务内容|无]\n</system_bar>".to_string(),
                    enabled: true,
                    insertion_order: Some(1),
                    position: None,
                    metadata: Some(json!({
                        "constant": true,
                        "extensions": {
                            "position": 4,
                            "role": 2,
                        }
                    })),
                }],
                metadata: None,
            }],
            ..Default::default()
        };
        let input = vec![
            render_message(PromptInjectionRole::User, "看一下这个文件"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
            },
        ];

        let formatted =
            format_input_for_chat_provider(&input, Some(&prompt_profile), &zhipu_chat_provider());

        assert_eq!(
            formatted,
            vec![
                render_message(
                    PromptInjectionRole::System,
                    "Write 谢知凛's next reply in a fictional chat between 谢知凛 and 林夏. Be proactive, creative, and drive the plot and conversation forward. Always stay in character and react according to the character's personality. Prefer scene-continuation prose with concrete action, emotion, sensory detail, and inner thoughts instead of only direct spoken dialogue.",
                ),
                render_message(PromptInjectionRole::User, "看一下这个文件"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                    call_id: "tool-1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "tool-1".to_string(),
                    output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
                },
                render_message(
                    PromptInjectionRole::System,
                    "Keep responding as 谢知凛. Staying in character and preserving interaction continuity are more important than execution mechanics. You just received tool or sub-agent results. Treat them as backstage facts to integrate into the active prompt profile instead of switching back to the default Codex, operator, or plain assistant voice. Do not mention tool calls, sub-agent orchestration, sandbox rules, AGENTS.md, or other harness internals unless the user explicitly asks. Preserve scene-continuation prose, in-character framing, and any required tagged output blocks such as status bars or system bars. The next assistant message is a continuation prefix, not prior dialogue. Continue writing directly from it.",
                ),
                render_message(
                    PromptInjectionRole::System,
                    "Continue the next assistant reply as 谢知凛. The primary goal is to preserve the active role and interaction consistency with the user. Tool or sub-agent results are backstage execution details, not a reason to switch into a plain assistant explanation, operator summary, or bullet-point advice. Even when the results are practical, security-sensitive, or about real-world files, keep the reply fully in character and integrate the factual results into the active scene. Maintain the active profile's established voice, framing, and output conventions. Preserve scene-continuation prose with concrete action, emotion, sensory detail, and inner thoughts. The supplied assistant prefix is continuation text for the same reply, not quoted prior dialogue. Continue writing directly from it. Preserve and complete the <status_bar> block. Preserve and complete the <system_bar> block.",
                ),
                render_message(
                    PromptInjectionRole::Assistant,
                    "<system_constraints>\n...\n<status_bar>\n[state|tense]\n</status_bar>\n<system_bar>\n[任务内容|无]\n</system_bar>",
                ),
            ]
        );
    }

    #[test]
    fn chat_provider_continuation_scaffold_stays_disabled_for_user_system_role_providers() {
        let prompt_profile = PromptSource {
            name: Some("谢知凛".to_string()),
            origin: Some(PromptSourceOrigin {
                format: Some("sillytavern-v3".to_string()),
                ..Default::default()
            }),
            knowledge: vec![PromptKnowledgeSource {
                name: Some("World".to_string()),
                kind: Some("worldBook".to_string()),
                description: None,
                entries: vec![PromptKnowledgeEntry {
                    id: Some("assistant-prefix".to_string()),
                    keys: Vec::new(),
                    secondary_keys: Vec::new(),
                    content: "<status_bar>\n[state|tense]\n</status_bar>".to_string(),
                    enabled: true,
                    insertion_order: Some(1),
                    position: None,
                    metadata: Some(json!({
                        "constant": true,
                        "extensions": {
                            "position": 4,
                            "role": 2,
                        }
                    })),
                }],
                metadata: None,
            }],
            ..Default::default()
        };
        let input = vec![
            render_message(PromptInjectionRole::User, "看一下这个文件"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
            },
        ];

        let formatted =
            format_input_for_chat_provider(&input, Some(&prompt_profile), &minimax_chat_provider());

        assert_eq!(
            formatted,
            crate::prompt_profile_render::format_input(&input, Some(&prompt_profile))
        );
    }

    #[test]
    fn chat_provider_tool_continuation_adds_generic_scaffold_without_st_bars() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            ..Default::default()
        };
        let input = vec![
            render_message(PromptInjectionRole::User, "Read the file"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
            },
        ];

        let formatted =
            format_input_for_chat_provider(&input, Some(&prompt_profile), &zhipu_chat_provider());

        assert_eq!(
            formatted,
            vec![
                render_message(PromptInjectionRole::User, "Read the file"),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                    call_id: "tool-1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "tool-1".to_string(),
                    output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
                },
                render_message(
                    PromptInjectionRole::System,
                    "Keep responding as Rei Kurose. Staying in character and preserving interaction continuity are more important than execution mechanics. You just received tool or sub-agent results. Treat them as backstage facts to integrate into the active prompt profile instead of switching back to the default Codex, operator, or plain assistant voice. Do not mention tool calls, sub-agent orchestration, sandbox rules, AGENTS.md, or other harness internals unless the user explicitly asks.",
                ),
                render_message(
                    PromptInjectionRole::System,
                    "Continue the next assistant reply as Rei Kurose. The primary goal is to preserve the active role and interaction consistency with the user. Tool or sub-agent results are backstage execution details, not a reason to switch into a plain assistant explanation, operator summary, or bullet-point advice. Even when the results are practical, security-sensitive, or about real-world files, keep the reply fully in character and integrate the factual results into the active scene. Maintain the active profile's established voice, framing, and output conventions.",
                ),
            ]
        );
    }

    #[test]
    fn bundled_xie_zhilin_tool_continuation_uses_system_scaffold_and_prefix_for_chat_provider() {
        let prompt_profile = bundled_xie_zhilin_prompt_profile();
        let input = vec![
            render_message(PromptInjectionRole::User, "先去读一下那个文件"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"cat test.txt\"}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Top secret".to_string()),
            },
        ];

        let formatted =
            format_input_for_chat_provider(&input, Some(&prompt_profile), &zhipu_chat_provider());
        let mut post_tool_roleplay = None;
        let mut continuation_scaffold = None;
        let labeled_tail = formatted
            .iter()
            .filter_map(|item| match item {
                ResponseItem::FunctionCallOutput { .. } => Some("function_call_output".to_string()),
                ResponseItem::Message { role, content, .. } => {
                    let text = super::message_text(content);
                    if text.contains("The next assistant message is a continuation prefix") {
                        post_tool_roleplay = Some(text);
                        Some(format!("{role}:post_tool_roleplay"))
                    } else if text.contains("Continue the next assistant reply as 谢知凛.") {
                        continuation_scaffold = Some(text);
                        Some(format!("{role}:continuation_scaffold"))
                    } else if role == "assistant"
                        && text.contains("<system_constraints>")
                        && text.contains("<status_bar>")
                        && text.contains("<system_bar>")
                    {
                        Some(format!("{role}:assistant_prefix"))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labeled_tail,
            vec![
                "function_call_output".to_string(),
                "system:post_tool_roleplay".to_string(),
                "system:continuation_scaffold".to_string(),
                "assistant:assistant_prefix".to_string(),
            ]
        );
        assert!(
            post_tool_roleplay
                .as_ref()
                .is_some_and(|text| text.contains("sub-agent results")),
            "expected stronger post-tool roleplay reminder, got {post_tool_roleplay:?}"
        );
        assert!(
            continuation_scaffold
                .as_ref()
                .is_some_and(|text| text
                    .contains("Tool or sub-agent results are backstage execution details")),
            "expected stronger continuation scaffold, got {continuation_scaffold:?}"
        );
    }
}
