use std::collections::BTreeSet;

use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_protocol::models::BASE_INSTRUCTIONS_DEFAULT;
use codex_protocol::models::BASE_INSTRUCTIONS_RUNTIME_CONTRACT;
use codex_protocol::models::BASE_INSTRUCTIONS_RUNTIME_CONTRACT_TEXT_EDITOR;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::prompt_profile::PromptDepthPrompt;
use codex_protocol::prompt_profile::PromptExampleMessage;
use codex_protocol::prompt_profile::PromptGreetingKind;
use codex_protocol::prompt_profile::PromptInjectionRole;
use codex_protocol::prompt_profile::PromptKnowledgeEntry;
use codex_protocol::prompt_profile::PromptSource;
use regex_lite::Regex;
use serde_json::Value as JsonValue;

const CORE_BASE_INSTRUCTIONS_DEFAULT: &str = include_str!("../prompt.md");
const CORE_BASE_INSTRUCTIONS_WITH_TEXT_EDITOR: &str =
    include_str!("../prompt_with_text_editor_instructions.md");
const CORE_DEFAULT_PERSONALITY_HEADER: &str = "You are Codex, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
const ACTIVE_CARD_PROMPT_START: &str = "<active_card_prompt>\n";
const ACTIVE_CARD_PROMPT_END: &str = "\n</active_card_prompt>";

#[derive(Clone, Copy, PartialEq, Eq)]
enum LorePosition {
    BeforeChar,
    AfterChar,
    Late,
}

#[derive(Default)]
struct ActiveLoreSections {
    before_char: Vec<String>,
    after_char: Vec<String>,
}

struct ActiveLateLoreEntry {
    role: PromptInjectionRole,
    depth: u32,
    insertion_order: i64,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatToolContinuationBridge {
    pub post_tool_roleplay: String,
    pub continuation_scaffold: String,
    pub assistant_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PromptProfileRenderOptions {
    pub(crate) subagent_delegation_hint: bool,
}

pub(crate) fn compose_base_instructions(
    base_instructions: &str,
    prompt_profile: Option<&PromptSource>,
) -> String {
    format_instructions(
        base_instructions,
        prompt_profile,
        &[],
        PromptProfileRenderOptions::default(),
    )
}

pub(crate) fn format_instructions(
    base_instructions: &str,
    prompt_profile: Option<&PromptSource>,
    input: &[ResponseItem],
    render_options: PromptProfileRenderOptions,
) -> String {
    let Some(prompt_profile) = prompt_profile else {
        return base_instructions.to_string();
    };
    let Some(rendered_profile) = render_profile_system_block(prompt_profile, input, render_options)
    else {
        return base_instructions.to_string();
    };
    if let Some(replaced) = replace_active_card_prompt_block(base_instructions, &rendered_profile) {
        return replaced;
    }
    let runtime_contract = builtin_runtime_contract(base_instructions).unwrap_or(base_instructions);
    format!("{runtime_contract}\n\n<active_card_prompt>\n{rendered_profile}\n</active_card_prompt>")
}

fn builtin_runtime_contract(base_instructions: &str) -> Option<&'static str> {
    if base_instructions == BASE_INSTRUCTIONS_DEFAULT
        || base_instructions == CORE_BASE_INSTRUCTIONS_DEFAULT
    {
        return Some(BASE_INSTRUCTIONS_RUNTIME_CONTRACT);
    }

    if base_instructions == CORE_BASE_INSTRUCTIONS_WITH_TEXT_EDITOR {
        return Some(BASE_INSTRUCTIONS_RUNTIME_CONTRACT_TEXT_EDITOR);
    }

    if base_instructions.starts_with(CORE_DEFAULT_PERSONALITY_HEADER) {
        if base_instructions.contains("`text_editor` tool")
            || base_instructions.contains("Use the `text_editor` tool to edit files.")
        {
            return Some(BASE_INSTRUCTIONS_RUNTIME_CONTRACT_TEXT_EDITOR);
        }
        return Some(BASE_INSTRUCTIONS_RUNTIME_CONTRACT);
    }

    None
}

pub(crate) fn format_input(
    input: &[ResponseItem],
    prompt_profile: Option<&PromptSource>,
) -> Vec<ResponseItem> {
    let Some(prompt_profile) = prompt_profile else {
        return input.to_vec();
    };
    if input.is_empty() {
        return Vec::new();
    }

    let runtime_head_items = build_runtime_head_items(prompt_profile);
    let example_items = build_example_items(Some(prompt_profile));
    let runtime_tail_items = build_runtime_tail_items(input, prompt_profile);
    if runtime_head_items.is_empty() && example_items.is_empty() && runtime_tail_items.is_empty() {
        return input.to_vec();
    }

    let split_index = if last_item_is_visible_user_message(input) {
        input.len().saturating_sub(1)
    } else {
        input.len()
    };
    let mut formatted = Vec::with_capacity(
        input.len() + runtime_head_items.len() + example_items.len() + runtime_tail_items.len(),
    );
    formatted.extend(runtime_head_items);
    formatted.extend(example_items);
    formatted.extend(input[..split_index].iter().cloned());
    formatted.extend(runtime_tail_items);
    formatted.extend(input[split_index..].iter().cloned());
    formatted
}

fn build_runtime_head_items(prompt_profile: &PromptSource) -> Vec<ResponseItem> {
    build_st_narrative_instruction(prompt_profile)
        .into_iter()
        .collect()
}

pub(crate) fn build_example_items(prompt_profile: Option<&PromptSource>) -> Vec<ResponseItem> {
    let Some(prompt_profile) = prompt_profile else {
        return Vec::new();
    };

    prompt_profile
        .examples
        .iter()
        .flat_map(|example| example.messages.iter())
        .map(|message| render_example_message(message, prompt_profile))
        .collect()
}

pub(crate) fn render_post_history_instructions(
    prompt_profile: Option<&PromptSource>,
) -> Option<String> {
    let prompt_profile = prompt_profile?;
    prompt_profile
        .post_history_instructions
        .as_deref()
        .map(|text| render_template(text, prompt_profile, None))
        .filter(|text| !text.trim().is_empty())
}

pub(crate) fn build_primary_greeting_item(
    prompt_profile: Option<&PromptSource>,
) -> Option<ResponseItem> {
    let prompt_profile = prompt_profile?;
    let greeting = prompt_profile
        .greetings
        .iter()
        .find(|greeting| greeting.kind == PromptGreetingKind::Primary)
        .or_else(|| prompt_profile.greetings.first())?;
    let greeting_text = render_template(&greeting.text, prompt_profile, None);
    (!greeting_text.trim().is_empty())
        .then(|| render_message(PromptInjectionRole::Assistant, greeting_text))
}

fn build_runtime_tail_items(
    input: &[ResponseItem],
    prompt_profile: &PromptSource,
) -> Vec<ResponseItem> {
    let mut items = Vec::new();
    let late_assistant_prefill = render_late_assistant_prefix(prompt_profile, input);

    for late_lore in active_late_lore_entries(prompt_profile, input) {
        if late_lore.role == PromptInjectionRole::Assistant {
        } else {
            items.push(render_message(late_lore.role, late_lore.content));
        }
    }

    if let Some(post_history_instructions) = render_post_history_instructions(Some(prompt_profile))
    {
        items.push(render_message(
            PromptInjectionRole::Developer,
            post_history_instructions,
        ));
    }

    if let Some(depth_prompt) = render_depth_prompt(input, prompt_profile) {
        items.push(depth_prompt);
    }

    if let Some(post_tool_roleplay_reminder) = render_tool_continuation_roleplay_reminder(
        input,
        prompt_profile,
        late_assistant_prefill.is_some(),
    ) {
        items.push(render_message(
            PromptInjectionRole::Developer,
            post_tool_roleplay_reminder,
        ));
    }

    if let Some(late_assistant_prefill) = late_assistant_prefill {
        items.push(render_message(
            PromptInjectionRole::Assistant,
            late_assistant_prefill,
        ));
    }

    items
}

pub(crate) fn build_chat_tool_continuation_bridge(
    input: &[ResponseItem],
    prompt_profile: &PromptSource,
) -> Option<ChatToolContinuationBridge> {
    if !is_tool_continuation_request(input) {
        return None;
    }

    let assistant_prefix = render_late_assistant_prefix(prompt_profile, input);
    let post_tool_roleplay = render_tool_continuation_roleplay_reminder(
        input,
        prompt_profile,
        assistant_prefix.is_some(),
    )?;
    let continuation_scaffold =
        render_chat_tool_continuation_scaffold(prompt_profile, assistant_prefix.as_deref());

    Some(ChatToolContinuationBridge {
        post_tool_roleplay,
        continuation_scaffold,
        assistant_prefix,
    })
}

fn render_depth_prompt(
    input: &[ResponseItem],
    prompt_profile: &PromptSource,
) -> Option<ResponseItem> {
    let depth_prompt = prompt_profile.depth_prompt.as_ref()?;
    let visible_user_turns = visible_user_turns(input);
    (visible_user_turns >= usize::try_from(depth_prompt.depth).ok()?)
        .then(|| render_depth_prompt_message(depth_prompt, prompt_profile))
}

fn render_depth_prompt_message(
    depth_prompt: &PromptDepthPrompt,
    prompt_profile: &PromptSource,
) -> ResponseItem {
    render_message(
        depth_prompt.role,
        render_template(&depth_prompt.content, prompt_profile, None),
    )
}

fn visible_user_turns(input: &[ResponseItem]) -> usize {
    input
        .iter()
        .filter(|item| matches!(crate::parse_turn_item(item), Some(TurnItem::UserMessage(_))))
        .count()
}

fn last_item_is_visible_user_message(input: &[ResponseItem]) -> bool {
    input
        .last()
        .is_some_and(|item| matches!(crate::parse_turn_item(item), Some(TurnItem::UserMessage(_))))
}

fn render_profile_system_block(
    prompt_profile: &PromptSource,
    input: &[ResponseItem],
    render_options: PromptProfileRenderOptions,
) -> Option<String> {
    let profile_body = render_profile_body(prompt_profile, input, render_options);
    let overlay_template = prompt_profile.system_overlay.as_deref();
    let overlay = overlay_template
        .map(|text| render_template(text, prompt_profile, profile_body.as_deref()))
        .filter(|text| !text.trim().is_empty());

    match (profile_body, overlay) {
        (Some(profile_body), Some(overlay)) => {
            if overlay_template.is_some_and(|text| text.contains("{{original}}")) {
                Some(overlay)
            } else {
                Some(format!("{overlay}\n{profile_body}"))
            }
        }
        (Some(profile_body), None) => Some(profile_body),
        (None, Some(overlay)) => Some(overlay),
        (None, None) => None,
    }
}

fn render_profile_body(
    prompt_profile: &PromptSource,
    input: &[ResponseItem],
    render_options: PromptProfileRenderOptions,
) -> Option<String> {
    let active_lore = active_lore_sections(prompt_profile, input);
    let mut sections = Vec::new();
    sections.extend(active_lore.before_char);
    if let Some(name) = prompt_profile_name(prompt_profile) {
        sections.push(format!("Name: {name}"));
    }
    if let Some(identity) = &prompt_profile.identity {
        if let Some(description) = &identity.description {
            sections.push(format!(
                "Description: {}",
                render_template(description, prompt_profile, None)
            ));
        }
        if let Some(personality) = &identity.personality {
            sections.push(format!(
                "Personality: {}",
                render_template(personality, prompt_profile, None)
            ));
        }
    }
    if let Some(scenario) = &prompt_profile.scenario {
        sections.push(format!(
            "Scenario: {}",
            render_template(scenario, prompt_profile, None)
        ));
    }
    if let Some(creator_notes) = &prompt_profile.creator_notes {
        sections.push(format!(
            "Creator Notes: {}",
            render_template(creator_notes, prompt_profile, None)
        ));
    }
    sections.extend(active_lore.after_char);
    if render_options.subagent_delegation_hint {
        sections.push(render_subagent_delegation_priority_rule(prompt_profile));
    }
    (!sections.is_empty()).then(|| sections.join("\n"))
}

fn render_subagent_delegation_priority_rule(prompt_profile: &PromptSource) -> String {
    let char_name = prompt_profile_name(prompt_profile).unwrap_or("the active character");
    format!(
        "Roleplay Priority: Preserving {char_name}'s role, voice, and interaction continuity with the user is the highest-priority objective. Do not go out of character for speed, convenience, task completion, or tool efficiency.
Delegation Rule: When sub-agents are available, delegate the user's actionable work to a sub-agent by default, especially for tool use, file operations, coding, search, execution, and multi-step analysis.
Main Session Role: Focus on the user-facing conversation as {char_name}. Let sub-agents handle backstage execution, then integrate their results into a final in-character reply.
Conflict Rule: If completing the task in the main session would risk breaking character or disrupting interaction consistency, protect character continuity first and treat task completion details as secondary."
    )
}

fn replace_active_card_prompt_block(
    base_instructions: &str,
    rendered_profile: &str,
) -> Option<String> {
    let start = base_instructions.find(ACTIVE_CARD_PROMPT_START)?;
    let content_start = start + ACTIVE_CARD_PROMPT_START.len();
    let end = base_instructions[content_start..].find(ACTIVE_CARD_PROMPT_END)?;
    let content_end = content_start + end;
    Some(format!(
        "{}{}{}",
        &base_instructions[..content_start],
        rendered_profile,
        &base_instructions[content_end..]
    ))
}

fn active_lore_sections(
    prompt_profile: &PromptSource,
    input: &[ResponseItem],
) -> ActiveLoreSections {
    let mut activated_entries = resolved_active_lore_entries(prompt_profile, input)
        .into_iter()
        .filter_map(|(entry, position)| match position {
            LorePosition::BeforeChar | LorePosition::AfterChar => Some((
                position,
                entry.insertion_order.unwrap_or_default(),
                render_template(&entry.content, prompt_profile, None),
            )),
            LorePosition::Late => None,
        })
        .filter(|(_, _, content)| !content.trim().is_empty())
        .collect::<Vec<_>>();
    activated_entries.sort_by_key(|(position, insertion_order, content)| {
        let position_order = match position {
            LorePosition::BeforeChar => 0_i32,
            LorePosition::AfterChar => 1_i32,
            LorePosition::Late => 2_i32,
        };
        (position_order, *insertion_order, content.clone())
    });

    let mut sections = ActiveLoreSections::default();
    let mut seen = BTreeSet::new();
    for (position, _, content) in activated_entries {
        if !seen.insert(content.clone()) {
            continue;
        }
        match position {
            LorePosition::BeforeChar => sections.before_char.push(content),
            LorePosition::AfterChar => sections.after_char.push(content),
            LorePosition::Late => {}
        }
    }
    sections
}

fn active_late_lore_entries(
    prompt_profile: &PromptSource,
    input: &[ResponseItem],
) -> Vec<ActiveLateLoreEntry> {
    let visible_user_turns = visible_user_turns(input);
    let mut entries = resolved_active_lore_entries(prompt_profile, input)
        .into_iter()
        .filter_map(|(entry, position)| (position == LorePosition::Late).then_some(entry))
        .filter_map(|entry| {
            let depth = lore_injection_depth(entry);
            (visible_user_turns >= usize::try_from(depth).ok()?).then_some(ActiveLateLoreEntry {
                role: lore_injection_role(entry),
                depth,
                insertion_order: entry.insertion_order.unwrap_or_default(),
                content: render_template(&entry.content, prompt_profile, None),
            })
        })
        .filter(|entry| !entry.content.trim().is_empty())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        let role_order = match entry.role {
            PromptInjectionRole::System => 0_i32,
            PromptInjectionRole::Developer => 1_i32,
            PromptInjectionRole::User => 2_i32,
            PromptInjectionRole::Assistant => 3_i32,
        };
        (
            entry.depth,
            role_order,
            entry.insertion_order,
            entry.content.clone(),
        )
    });
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|entry| {
            let role_key = match entry.role {
                PromptInjectionRole::System => 0_i32,
                PromptInjectionRole::Developer => 1_i32,
                PromptInjectionRole::User => 2_i32,
                PromptInjectionRole::Assistant => 3_i32,
            };
            seen.insert((role_key, entry.content.clone()))
        })
        .collect()
}

fn resolved_active_lore_entries<'a>(
    prompt_profile: &'a PromptSource,
    input: &[ResponseItem],
) -> Vec<(&'a PromptKnowledgeEntry, LorePosition)> {
    let lore_match_text = lore_match_text(input);
    let lore_match_text_lower = lore_match_text.to_lowercase();
    renderable_knowledge_sources(prompt_profile)
        .into_iter()
        .flat_map(|source| source.entries.iter())
        .filter(|entry| {
            lore_entry_is_active(
                entry,
                &lore_match_text,
                &lore_match_text_lower,
                prompt_profile,
            )
        })
        .filter_map(|entry| lore_position(entry).map(|position| (entry, position)))
        .collect()
}

fn renderable_knowledge_sources(
    prompt_profile: &PromptSource,
) -> Vec<&codex_protocol::prompt_profile::PromptKnowledgeSource> {
    let world_book_names = prompt_profile
        .knowledge
        .iter()
        .filter(|source| source.kind.as_deref() == Some("worldBook"))
        .filter_map(|source| source.name.as_deref())
        .collect::<BTreeSet<_>>();
    prompt_profile
        .knowledge
        .iter()
        .filter(|source| !source.entries.is_empty())
        .filter(|source| {
            !(source.kind.as_deref() == Some("characterBook")
                && source
                    .name
                    .as_deref()
                    .is_some_and(|name| world_book_names.contains(name)))
        })
        .collect()
}

fn lore_match_text(input: &[ResponseItem]) -> String {
    input
        .iter()
        .filter_map(|item| match crate::parse_turn_item(item) {
            Some(TurnItem::UserMessage(message)) => Some(message.message()),
            Some(TurnItem::AgentMessage(message)) => Some(
                message
                    .content
                    .iter()
                    .map(|content| match content {
                        AgentMessageContent::Text { text } => text.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lore_entry_is_active(
    entry: &PromptKnowledgeEntry,
    lore_match_text: &str,
    lore_match_text_lower: &str,
    prompt_profile: &PromptSource,
) -> bool {
    if !entry.enabled {
        return false;
    }
    if metadata_bool(entry.metadata.as_ref(), "constant") {
        return true;
    }

    let primary_match = lore_keys_match(
        &entry.keys,
        lore_match_text,
        lore_match_text_lower,
        entry.metadata.as_ref(),
        prompt_profile,
    );
    let secondary_match = lore_keys_match(
        &entry.secondary_keys,
        lore_match_text,
        lore_match_text_lower,
        entry.metadata.as_ref(),
        prompt_profile,
    );
    if metadata_bool(entry.metadata.as_ref(), "selective") && !entry.secondary_keys.is_empty() {
        primary_match && secondary_match
    } else {
        primary_match || secondary_match
    }
}

fn build_st_narrative_instruction(prompt_profile: &PromptSource) -> Option<ResponseItem> {
    if !prompt_profile_is_sillytavern(prompt_profile) {
        return None;
    }
    let char_name = prompt_profile_name(prompt_profile)?;
    let user_name = prompt_profile_user_name(prompt_profile);
    Some(render_message(
        PromptInjectionRole::System,
        format!(
            "Write {char_name}'s next reply in a fictional chat between {char_name} and {user_name}. Be proactive, creative, and drive the plot and conversation forward. Always stay in character and react according to the character's personality. Prefer scene-continuation prose with concrete action, emotion, sensory detail, and inner thoughts instead of only direct spoken dialogue."
        ),
    ))
}

fn prompt_profile_is_sillytavern(prompt_profile: &PromptSource) -> bool {
    prompt_profile
        .origin
        .as_ref()
        .and_then(|origin| origin.format.as_deref())
        .is_some_and(|format| format.starts_with("sillytavern"))
}

pub(crate) fn render_tool_continuation_roleplay_reminder(
    input: &[ResponseItem],
    prompt_profile: &PromptSource,
    has_assistant_prefix: bool,
) -> Option<String> {
    if !is_tool_continuation_request(input) {
        return None;
    }

    let char_name = prompt_profile_name(prompt_profile).unwrap_or("the active character");
    let mut reminder = format!(
        "Keep responding as {char_name}. Staying in character and preserving interaction continuity are more important than execution mechanics. You just received tool or sub-agent results. Treat them as backstage facts to integrate into the active prompt profile instead of switching back to the default Codex, operator, or plain assistant voice. Do not mention tool calls, sub-agent orchestration, sandbox rules, AGENTS.md, or other harness internals unless the user explicitly asks."
    );
    if prompt_profile_is_sillytavern(prompt_profile) {
        reminder.push_str(
            " Preserve scene-continuation prose, in-character framing, and any required tagged output blocks such as status bars or system bars.",
        );
    }
    if has_assistant_prefix {
        reminder.push_str(
            " The next assistant message is a continuation prefix, not prior dialogue. Continue writing directly from it.",
        );
    }
    Some(reminder)
}

fn render_late_assistant_prefix(
    prompt_profile: &PromptSource,
    input: &[ResponseItem],
) -> Option<String> {
    let prefix = active_late_lore_entries(prompt_profile, input)
        .into_iter()
        .filter(|entry| entry.role == PromptInjectionRole::Assistant)
        .map(|entry| entry.content)
        .collect::<Vec<_>>()
        .join("\n");

    (!prefix.trim().is_empty()).then_some(prefix)
}

fn render_chat_tool_continuation_scaffold(
    prompt_profile: &PromptSource,
    assistant_prefix: Option<&str>,
) -> String {
    let char_name = prompt_profile_name(prompt_profile).unwrap_or("the active character");
    let mut scaffold = format!(
        "Continue the next assistant reply as {char_name}. The primary goal is to preserve the active role and interaction consistency with the user. Tool or sub-agent results are backstage execution details, not a reason to switch into a plain assistant explanation, operator summary, or bullet-point advice. Even when the results are practical, security-sensitive, or about real-world files, keep the reply fully in character and integrate the factual results into the active scene. Maintain the active profile's established voice, framing, and output conventions."
    );

    if prompt_profile_is_sillytavern(prompt_profile) {
        scaffold.push_str(
            " Preserve scene-continuation prose with concrete action, emotion, sensory detail, and inner thoughts.",
        );
    }

    if let Some(assistant_prefix) = assistant_prefix {
        scaffold.push_str(
            " The supplied assistant prefix is continuation text for the same reply, not quoted prior dialogue. Continue writing directly from it.",
        );
        if assistant_prefix.contains("<status_bar>") {
            scaffold.push_str(" Preserve and complete the <status_bar> block.");
        }
        if assistant_prefix.contains("<system_bar>") {
            scaffold.push_str(" Preserve and complete the <system_bar> block.");
        }
    }

    scaffold
}

fn is_tool_continuation_request(input: &[ResponseItem]) -> bool {
    input
        .iter()
        .rev()
        .find_map(|item| match item {
            ResponseItem::FunctionCallOutput { .. } | ResponseItem::CustomToolCallOutput { .. } => {
                Some(true)
            }
            ResponseItem::Message { role, .. } if role == "user" || role == "assistant" => {
                Some(false)
            }
            _ => None,
        })
        .unwrap_or(false)
}

fn lore_keys_match(
    keys: &[String],
    lore_match_text: &str,
    lore_match_text_lower: &str,
    metadata: Option<&JsonValue>,
    prompt_profile: &PromptSource,
) -> bool {
    let use_regex = metadata_bool(metadata, "use_regex");
    let case_sensitive = metadata_bool(metadata, "case_sensitive")
        || metadata_nested_bool(metadata, "extensions", "case_sensitive");
    keys.iter().any(|key| {
        let rendered_key = render_template(key, prompt_profile, None);
        let trimmed = rendered_key.trim();
        !trimmed.is_empty()
            && lore_key_matches(
                trimmed,
                lore_match_text,
                lore_match_text_lower,
                use_regex,
                case_sensitive,
            )
    })
}

fn metadata_bool(metadata: Option<&JsonValue>, key: &str) -> bool {
    metadata
        .and_then(JsonValue::as_object)
        .and_then(|metadata| metadata.get(key))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn metadata_nested_i64(metadata: Option<&JsonValue>, object_key: &str, key: &str) -> Option<i64> {
    metadata
        .and_then(JsonValue::as_object)
        .and_then(|metadata| metadata.get(object_key))
        .and_then(JsonValue::as_object)
        .and_then(|metadata| metadata.get(key))
        .and_then(JsonValue::as_i64)
}

fn metadata_nested_bool(metadata: Option<&JsonValue>, object_key: &str, key: &str) -> bool {
    metadata
        .and_then(JsonValue::as_object)
        .and_then(|metadata| metadata.get(object_key))
        .and_then(JsonValue::as_object)
        .and_then(|metadata| metadata.get(key))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn lore_injection_depth(entry: &PromptKnowledgeEntry) -> u32 {
    metadata_nested_i64(entry.metadata.as_ref(), "extensions", "depth")
        .and_then(|depth| u32::try_from(depth).ok())
        .unwrap_or_default()
}

fn lore_injection_role(entry: &PromptKnowledgeEntry) -> PromptInjectionRole {
    match metadata_nested_i64(entry.metadata.as_ref(), "extensions", "role") {
        Some(0) => PromptInjectionRole::System,
        Some(1) => PromptInjectionRole::User,
        Some(2) => PromptInjectionRole::Assistant,
        _ => PromptInjectionRole::System,
    }
}

fn lore_key_matches(
    key: &str,
    lore_match_text: &str,
    lore_match_text_lower: &str,
    use_regex: bool,
    case_sensitive: bool,
) -> bool {
    if use_regex && let Some(regex) = compile_lore_regex(key, case_sensitive) {
        return regex.is_match(lore_match_text);
    }

    lore_match_text.contains(key) || lore_match_text_lower.contains(key.to_lowercase().as_str())
}

fn compile_lore_regex(pattern: &str, case_sensitive: bool) -> Option<Regex> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    let candidate = if case_sensitive || pattern.starts_with("(?") {
        pattern.to_string()
    } else {
        format!("(?i){pattern}")
    };
    Regex::new(&candidate).ok()
}

fn lore_position(entry: &PromptKnowledgeEntry) -> Option<LorePosition> {
    match metadata_nested_i64(entry.metadata.as_ref(), "extensions", "position") {
        Some(0) => return Some(LorePosition::BeforeChar),
        Some(1) => return Some(LorePosition::AfterChar),
        Some(4) => return Some(LorePosition::Late),
        Some(_) => return None,
        None => {}
    }
    match entry
        .position
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
    {
        Some(value) if value == "before_char" || value == "beforechar" => {
            Some(LorePosition::BeforeChar)
        }
        Some(value) if value == "after_char" || value == "afterchar" => {
            Some(LorePosition::AfterChar)
        }
        Some(value)
            if value == "late"
                || value == "late_depth"
                || value == "at_depth"
                || value == "atdepth" =>
        {
            Some(LorePosition::Late)
        }
        Some(_) => None,
        None => Some(LorePosition::AfterChar),
    }
}

fn prompt_profile_name(prompt_profile: &PromptSource) -> Option<&str> {
    prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.name.as_deref())
        .or(prompt_profile.name.as_deref())
}

fn prompt_profile_user_name(prompt_profile: &PromptSource) -> &str {
    prompt_profile
        .variables
        .get("user_name")
        .or_else(|| prompt_profile.variables.get("user"))
        .map(String::as_str)
        .unwrap_or("User")
}

fn fallback_user_name_for_text(text: &str) -> &'static str {
    if text
        .chars()
        .any(|ch| ('\u{4E00}'..='\u{9FFF}').contains(&ch))
    {
        "你"
    } else {
        "User"
    }
}

fn render_template(text: &str, prompt_profile: &PromptSource, original: Option<&str>) -> String {
    let mut rendered = text.to_string();
    for (key, value) in &prompt_profile.variables {
        rendered = rendered.replace(format!("{{{{{key}}}}}").as_str(), value);
    }
    let user_name = prompt_profile
        .variables
        .get("user_name")
        .or_else(|| prompt_profile.variables.get("user"))
        .map(String::as_str)
        .unwrap_or_else(|| fallback_user_name_for_text(text));
    rendered = rendered.replace("{{user}}", user_name);
    if let Some(name) = prompt_profile_name(prompt_profile) {
        rendered = rendered.replace("{{char}}", name);
    }
    rendered = rendered.replace("{{original}}", original.unwrap_or(""));
    rendered
}

fn render_example_message(
    message: &PromptExampleMessage,
    prompt_profile: &PromptSource,
) -> ResponseItem {
    render_message(
        message.role,
        render_template(&message.content, prompt_profile, None),
    )
}

fn render_message(role: PromptInjectionRole, text: String) -> ResponseItem {
    let role = match role {
        PromptInjectionRole::System => "system",
        PromptInjectionRole::Developer => "developer",
        PromptInjectionRole::User => "user",
        PromptInjectionRole::Assistant => "assistant",
    };
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
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::prompt_profile::PromptExample;
    use codex_protocol::prompt_profile::PromptGreeting;
    use codex_protocol::prompt_profile::PromptIdentity;
    use codex_protocol::prompt_profile::PromptKnowledgeSource;
    use codex_protocol::prompt_profile::PromptSourceOrigin;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn compose_base_instructions_appends_prompt_profile_block() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: Some("Restrained and surgical.".to_string()),
            }),
            scenario: Some("Late-night pair debugging.".to_string()),
            system_overlay: Some("You are {{char}}.\n{{original}}".to_string()),
            ..Default::default()
        };

        let composed = compose_base_instructions("runtime contract", Some(&prompt_profile));

        assert_eq!(
            composed,
            "runtime contract\n\n<active_card_prompt>\nYou are Rei Kurose.\nName: Rei Kurose\nDescription: A quiet late-night engineering companion.\nPersonality: Restrained and surgical.\nScenario: Late-night pair debugging.\n</active_card_prompt>"
        );
    }

    #[test]
    fn compose_base_instructions_preserves_profile_body_without_original_placeholder() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: None,
            }),
            system_overlay: Some("You are {{char}}.".to_string()),
            ..Default::default()
        };

        let composed = compose_base_instructions("runtime contract", Some(&prompt_profile));

        assert_eq!(
            composed,
            "runtime contract\n\n<active_card_prompt>\nYou are Rei Kurose.\nName: Rei Kurose\nDescription: A quiet late-night engineering companion.\n</active_card_prompt>"
        );
    }

    #[test]
    fn compose_base_instructions_replaces_default_base_with_runtime_contract() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: None,
            }),
            ..Default::default()
        };

        let composed = compose_base_instructions(BASE_INSTRUCTIONS_DEFAULT, Some(&prompt_profile));

        assert!(
            composed.contains("<active_card_prompt>"),
            "expected active card prompt wrapper"
        );
        assert!(
            composed.contains(BASE_INSTRUCTIONS_RUNTIME_CONTRACT.trim()),
            "expected runtime contract when a prompt profile is active"
        );
        assert!(
            !composed
                .contains("Your default personality and tone is concise, direct, and friendly."),
            "default Codex persona should not remain when a prompt profile is active"
        );
    }

    #[test]
    fn compose_base_instructions_replaces_core_personality_wrapped_base_with_runtime_contract() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: None,
            }),
            ..Default::default()
        };

        let wrapped_base_instructions = format!(
            "{CORE_DEFAULT_PERSONALITY_HEADER}\n\n## Personality\n\nDefault personality block.\n\n{CORE_BASE_INSTRUCTIONS_DEFAULT}"
        );
        let composed = compose_base_instructions(&wrapped_base_instructions, Some(&prompt_profile));

        assert!(
            composed.contains(BASE_INSTRUCTIONS_RUNTIME_CONTRACT.trim()),
            "expected apply-patch runtime contract for personality-wrapped core base instructions"
        );
        assert!(
            !composed.contains("Default personality block."),
            "wrapped Codex persona should not remain when a prompt profile is active"
        );
    }

    #[test]
    fn compose_base_instructions_replaces_text_editor_base_with_text_editor_runtime_contract() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: None,
            }),
            ..Default::default()
        };

        let composed = compose_base_instructions(
            CORE_BASE_INSTRUCTIONS_WITH_TEXT_EDITOR,
            Some(&prompt_profile),
        );

        assert!(
            composed.contains(BASE_INSTRUCTIONS_RUNTIME_CONTRACT_TEXT_EDITOR.trim()),
            "expected text-editor runtime contract when a prompt profile is active"
        );
        assert!(
            !composed
                .contains("Your default personality and tone is concise, direct, and friendly."),
            "text-editor Codex persona should not remain when a prompt profile is active"
        );
    }

    #[test]
    fn format_instructions_appends_subagent_delegation_priority_rule_when_enabled() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            identity: Some(PromptIdentity {
                name: Some("Rei Kurose".to_string()),
                description: Some("A quiet late-night engineering companion.".to_string()),
                personality: None,
            }),
            ..Default::default()
        };

        let formatted = format_instructions(
            "runtime contract",
            Some(&prompt_profile),
            &[],
            PromptProfileRenderOptions {
                subagent_delegation_hint: true,
            },
        );

        assert_eq!(formatted.matches("Roleplay Priority:").count(), 1);
        assert!(
            formatted.contains("Delegation Rule: When sub-agents are available, delegate the user's actionable work to a sub-agent by default"),
            "expected strong delegation rule in instructions, got {formatted}"
        );
    }

    #[test]
    fn format_instructions_skips_subagent_delegation_priority_rule_by_default() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            ..Default::default()
        };

        let formatted = format_instructions(
            "runtime contract",
            Some(&prompt_profile),
            &[],
            PromptProfileRenderOptions::default(),
        );

        assert!(
            !formatted.contains("Roleplay Priority:"),
            "did not expect delegation rule without render option, got {formatted}"
        );
    }

    #[test]
    fn format_instructions_keeps_subagent_delegation_rule_inside_overlay_with_original_placeholder()
    {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            system_overlay: Some("Overlay start\n{{original}}\nOverlay end".to_string()),
            ..Default::default()
        };

        let formatted = format_instructions(
            "runtime contract",
            Some(&prompt_profile),
            &[],
            PromptProfileRenderOptions {
                subagent_delegation_hint: true,
            },
        );

        assert_eq!(formatted.matches("Roleplay Priority:").count(), 1);
        assert!(
            formatted.contains(
                "<active_card_prompt>\nOverlay start\nName: Rei Kurose\nRoleplay Priority: Preserving Rei Kurose's role, voice, and interaction continuity with the user is the highest-priority objective."
            ),
            "expected delegation rule to remain inside overlay-wrapped active card prompt, got {formatted}"
        );
        assert!(
            formatted.contains(
                "Conflict Rule: If completing the task in the main session would risk breaking character or disrupting interaction consistency, protect character continuity first and treat task completion details as secondary.\nOverlay end\n</active_card_prompt>"
            ),
            "expected overlay to wrap the full delegation rule, got {formatted}"
        );
    }

    #[test]
    fn build_example_items_preserves_roles() {
        let prompt_profile = PromptSource {
            examples: vec![PromptExample {
                messages: vec![
                    PromptExampleMessage {
                        role: PromptInjectionRole::User,
                        content: "hello".to_string(),
                    },
                    PromptExampleMessage {
                        role: PromptInjectionRole::Assistant,
                        content: "world".to_string(),
                    },
                ],
            }],
            depth_prompt: Some(PromptDepthPrompt {
                depth: 4,
                role: PromptInjectionRole::Developer,
                content: "later {{char}}".to_string(),
            }),
            greetings: vec![PromptGreeting {
                kind: PromptGreetingKind::Primary,
                text: "hi".to_string(),
            }],
            name: Some("Rei Kurose".to_string()),
            ..Default::default()
        };

        let items = build_example_items(Some(&prompt_profile));

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hello".to_string(),
                }],
                end_turn: None,
                phase: None,
            }
        );
        assert_eq!(
            items[1],
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "world".to_string(),
                }],
                end_turn: None,
                phase: None,
            }
        );
    }

    #[test]
    fn format_input_inserts_examples_and_tail_around_last_visible_user_message() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            post_history_instructions: Some("Stay in character as {{char}}.".to_string()),
            examples: vec![PromptExample {
                messages: vec![
                    PromptExampleMessage {
                        role: PromptInjectionRole::User,
                        content: "Example user".to_string(),
                    },
                    PromptExampleMessage {
                        role: PromptInjectionRole::Assistant,
                        content: "Example assistant".to_string(),
                    },
                ],
            }],
            depth_prompt: Some(PromptDepthPrompt {
                depth: 1,
                role: PromptInjectionRole::Developer,
                content: "Depth prompt for {{char}}.".to_string(),
            }),
            ..Default::default()
        };
        let input = vec![
            render_message(PromptInjectionRole::Assistant, "Seed greeting".to_string()),
            render_message(
                PromptInjectionRole::User,
                "Current user message".to_string(),
            ),
        ];

        let formatted = format_input(&input, Some(&prompt_profile));

        assert_eq!(
            formatted,
            vec![
                render_message(PromptInjectionRole::User, "Example user".to_string()),
                render_message(
                    PromptInjectionRole::Assistant,
                    "Example assistant".to_string(),
                ),
                render_message(PromptInjectionRole::Assistant, "Seed greeting".to_string(),),
                render_message(
                    PromptInjectionRole::Developer,
                    "Stay in character as Rei Kurose.".to_string(),
                ),
                render_message(
                    PromptInjectionRole::Developer,
                    "Depth prompt for Rei Kurose.".to_string(),
                ),
                render_message(
                    PromptInjectionRole::User,
                    "Current user message".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn build_primary_greeting_item_prefers_primary_kind() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            greetings: vec![
                PromptGreeting {
                    kind: PromptGreetingKind::Alternate,
                    text: "Alternate".to_string(),
                },
                PromptGreeting {
                    kind: PromptGreetingKind::Primary,
                    text: "Hello from {{char}}".to_string(),
                },
            ],
            ..Default::default()
        };

        let greeting = build_primary_greeting_item(Some(&prompt_profile));

        assert_eq!(
            greeting,
            Some(render_message(
                PromptInjectionRole::Assistant,
                "Hello from Rei Kurose".to_string(),
            ))
        );
    }

    #[test]
    fn render_post_history_instructions_substitutes_char_name() {
        let prompt_profile = PromptSource {
            name: Some("Rei Kurose".to_string()),
            post_history_instructions: Some("Stay in character as {{char}}.".to_string()),
            ..Default::default()
        };

        assert_eq!(
            render_post_history_instructions(Some(&prompt_profile)),
            Some("Stay in character as Rei Kurose.".to_string())
        );
    }

    #[test]
    fn render_post_history_instructions_defaults_user_placeholder_to_user() {
        let prompt_profile = PromptSource {
            name: Some("谢知凛".to_string()),
            post_history_instructions: Some("背着{{user}}往家走".to_string()),
            ..Default::default()
        };

        assert_eq!(
            render_post_history_instructions(Some(&prompt_profile)),
            Some("背着你往家走".to_string())
        );
    }

    #[test]
    fn format_input_tool_continuation_places_roleplay_reminder_before_assistant_prefix() {
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
            render_message(PromptInjectionRole::User, "看一下这个文件".to_string()),
            ResponseItem::FunctionCall {
                id: None,
                name: "update_plan".to_string(),
                arguments: "{}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Plan updated".to_string()),
            },
        ];

        let formatted = format_input(&input, Some(&prompt_profile));

        assert_eq!(
            formatted,
            vec![
                render_message(
                    PromptInjectionRole::System,
                    "Write 谢知凛's next reply in a fictional chat between 谢知凛 and 林夏. Be proactive, creative, and drive the plot and conversation forward. Always stay in character and react according to the character's personality. Prefer scene-continuation prose with concrete action, emotion, sensory detail, and inner thoughts instead of only direct spoken dialogue.".to_string(),
                ),
                render_message(PromptInjectionRole::User, "看一下这个文件".to_string()),
                ResponseItem::FunctionCall {
                    id: None,
                    name: "update_plan".to_string(),
                    arguments: "{}".to_string(),
                    call_id: "tool-1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "tool-1".to_string(),
                    output: FunctionCallOutputPayload::from_text("Plan updated".to_string()),
                },
                render_message(
                    PromptInjectionRole::Developer,
                    "Keep responding as 谢知凛. Staying in character and preserving interaction continuity are more important than execution mechanics. You just received tool or sub-agent results. Treat them as backstage facts to integrate into the active prompt profile instead of switching back to the default Codex, operator, or plain assistant voice. Do not mention tool calls, sub-agent orchestration, sandbox rules, AGENTS.md, or other harness internals unless the user explicitly asks. Preserve scene-continuation prose, in-character framing, and any required tagged output blocks such as status bars or system bars. The next assistant message is a continuation prefix, not prior dialogue. Continue writing directly from it.".to_string(),
                ),
                render_message(
                    PromptInjectionRole::Assistant,
                    "<status_bar>\n[state|tense]\n</status_bar>".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn tool_continuation_detection_stops_after_a_follow_up_user_message() {
        let input = vec![
            render_message(PromptInjectionRole::User, "先看看文件".to_string()),
            ResponseItem::FunctionCall {
                id: None,
                name: "update_plan".to_string(),
                arguments: "{}".to_string(),
                call_id: "tool-1".to_string(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "tool-1".to_string(),
                output: FunctionCallOutputPayload::from_text("Plan updated".to_string()),
            },
            render_message(PromptInjectionRole::User, "那你现在怎么看".to_string()),
        ];

        assert!(!is_tool_continuation_request(&input));
    }
}
