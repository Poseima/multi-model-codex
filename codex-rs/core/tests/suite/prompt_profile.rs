use anyhow::Context;
use anyhow::Result;
use codex_core::load_prompt_profile_from_path;
use codex_core::read_session_meta_line;
use codex_protocol::prompt_profile::PromptDepthPrompt;
use codex_protocol::prompt_profile::PromptExample;
use codex_protocol::prompt_profile::PromptExampleMessage;
use codex_protocol::prompt_profile::PromptGreeting;
use codex_protocol::prompt_profile::PromptGreetingKind;
use codex_protocol::prompt_profile::PromptIdentity;
use codex_protocol::prompt_profile::PromptInjectionRole;
use codex_protocol::prompt_profile::PromptKnowledgeEntry;
use codex_protocol::prompt_profile::PromptSource;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use codex_utils_cargo_bin::find_resource;
use core_test_support::context_snapshot;
use core_test_support::context_snapshot::ContextSnapshotOptions;
use core_test_support::context_snapshot::ContextSnapshotRenderMode;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use pretty_assertions::assert_eq;
use serde_json::Value as JsonValue;
use serde_json::Value;
use wiremock::MockServer;

fn bundled_xie_zhilin_png_fixture() -> Result<std::path::PathBuf> {
    find_resource!("tests/fixtures/xie_zhilin_card_v3.png")
        .context("failed to resolve bundled Xie Zhiling PNG fixture")
}

fn load_bundled_xie_zhilin_prompt_profile() -> Result<PromptSource> {
    let path = bundled_xie_zhilin_png_fixture()?;
    load_prompt_profile_from_path(&path)
}

fn request_message_entries(request: &ResponsesRequest) -> Vec<(String, String)> {
    request
        .input()
        .into_iter()
        .filter_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("message")).then_some(item)
        })
        .filter_map(|item| {
            let role = item.get("role").and_then(Value::as_str)?.to_string();
            let text = item
                .get("content")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(|content| content.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            Some((role, text))
        })
        .collect()
}

fn labeled_prompt_profile_order(request: &ResponsesRequest) -> Vec<String> {
    request_message_entries(request)
        .into_iter()
        .filter_map(|(role, text)| {
            if text.contains("Example user") {
                Some(format!("{role}:example_user"))
            } else if text.contains("Example assistant") {
                Some(format!("{role}:example_assistant"))
            } else if text.contains("The carriage is quiet tonight.") {
                Some(format!("{role}:greeting"))
            } else if text.contains("Stay in character as Rei Kurose.") {
                Some(format!("{role}:post_history"))
            } else if text.contains("Depth prompt for Rei Kurose.") {
                Some(format!("{role}:depth_prompt"))
            } else if text.contains("Review this parser.") {
                Some(format!("{role}:current_user"))
            } else {
                None
            }
        })
        .collect()
}

fn role_texts<'a>(entries: &'a [(String, String)], role: &str) -> Vec<&'a str> {
    entries
        .iter()
        .filter(|(entry_role, _)| entry_role == role)
        .map(|(_, text)| text.as_str())
        .collect()
}

fn all_message_texts(entries: &[(String, String)]) -> Vec<&str> {
    entries.iter().map(|(_, text)| text.as_str()).collect()
}

fn openai_responses_role(role: PromptInjectionRole) -> &'static str {
    match role {
        PromptInjectionRole::System | PromptInjectionRole::Developer => "developer",
        PromptInjectionRole::User => "user",
        PromptInjectionRole::Assistant => "assistant",
    }
}

fn prompt_profile_name(prompt_profile: &PromptSource) -> Option<&str> {
    prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.name.as_deref())
        .or(prompt_profile.name.as_deref())
}

fn render_prompt_profile_text(text: &str, prompt_profile: &PromptSource) -> String {
    let mut rendered = text.to_string();
    for (key, value) in &prompt_profile.variables {
        rendered = rendered.replace(format!("{{{{{key}}}}}").as_str(), value);
    }
    let fallback_user_name = if text
        .chars()
        .any(|ch| ('\u{4E00}'..='\u{9FFF}').contains(&ch))
    {
        "你"
    } else {
        "User"
    };
    let user_name = prompt_profile
        .variables
        .get("user_name")
        .or_else(|| prompt_profile.variables.get("user"))
        .map(String::as_str)
        .unwrap_or(fallback_user_name);
    rendered = rendered.replace("{{user}}", user_name);
    if let Some(name) = prompt_profile_name(prompt_profile) {
        rendered = rendered.replace("{{char}}", name);
    }
    rendered.replace("{{original}}", "")
}

fn prompt_profile_primary_greeting(prompt_profile: &PromptSource) -> Option<String> {
    prompt_profile
        .greetings
        .iter()
        .find(|greeting| greeting.kind == PromptGreetingKind::Primary)
        .or_else(|| prompt_profile.greetings.first())
        .map(|greeting| render_prompt_profile_text(&greeting.text, prompt_profile))
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

fn prompt_profile_renderable_knowledge_entries(
    prompt_profile: &PromptSource,
) -> Vec<&PromptKnowledgeEntry> {
    let world_book_names = prompt_profile
        .knowledge
        .iter()
        .filter(|source| source.kind.as_deref() == Some("worldBook"))
        .filter_map(|source| source.name.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
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
        .flat_map(|source| source.entries.iter())
        .collect()
}

fn prompt_profile_constant_lore_contents(prompt_profile: &PromptSource) -> Vec<String> {
    prompt_profile_renderable_knowledge_entries(prompt_profile)
        .into_iter()
        .filter(|entry| {
            entry.enabled
                && metadata_bool(entry.metadata.as_ref(), "constant")
                && metadata_nested_i64(entry.metadata.as_ref(), "extensions", "position")
                    .is_none_or(|position| matches!(position, 0 | 1))
        })
        .map(|entry| render_prompt_profile_text(&entry.content, prompt_profile))
        .collect()
}

fn prompt_profile_character_book_entries(
    prompt_profile: &PromptSource,
) -> Vec<&PromptKnowledgeEntry> {
    prompt_profile
        .knowledge
        .iter()
        .filter(|source| source.kind.as_deref() == Some("characterBook"))
        .flat_map(|source| source.entries.iter())
        .collect()
}

fn prompt_profile_world_book_entries(prompt_profile: &PromptSource) -> Vec<&PromptKnowledgeEntry> {
    prompt_profile
        .knowledge
        .iter()
        .filter(|source| source.kind.as_deref() == Some("worldBook"))
        .flat_map(|source| source.entries.iter())
        .collect()
}

async fn capture_last_request_for_prompt_profile_turns(
    prompt_profile: PromptSource,
    user_messages: &[&str],
) -> Result<ResponsesRequest> {
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        user_messages
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let response_id = format!("resp-{}", index + 1);
                let message_id = format!("msg-{}", index + 1);
                let reply = format!("turn {} complete", index + 1);
                sse(vec![
                    ev_response_created(&response_id),
                    ev_assistant_message(&message_id, &reply),
                    ev_completed(&response_id),
                ])
            })
            .collect(),
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2-codex")
        .build(&server)
        .await?;
    let new_thread = test
        .thread_manager
        .start_thread_with_tools(test.config.clone(), Vec::new(), Some(prompt_profile), false)
        .await?;

    for message in user_messages {
        new_thread
            .thread
            .submit(Op::UserTurn {
                items: vec![UserInput::Text {
                    text: (*message).to_string(),
                    text_elements: Vec::new(),
                }],
                final_output_json_schema: None,
                cwd: test.cwd_path().to_path_buf(),
                approval_policy: codex_protocol::protocol::AskForApproval::Never,
                sandbox_policy: SandboxPolicy::DangerFullAccess,
                model: new_thread.session_configured.model.clone(),
                effort: None,
                summary: None,
                service_tier: None,
                collaboration_mode: None,
                personality: None,
            })
            .await?;
        let turn_id = wait_for_event_match(&new_thread.thread, |event| match event {
            EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
            _ => None,
        })
        .await;
        wait_for_event(&new_thread.thread, |event| match event {
            EventMsg::TurnComplete(event) => event.turn_id == turn_id,
            _ => false,
        })
        .await;
    }

    responses
        .last_request()
        .context("last request should be captured")
}

async fn capture_fourth_request_for_prompt_profile(
    prompt_profile: PromptSource,
) -> Result<ResponsesRequest> {
    capture_last_request_for_prompt_profile_turns(
        prompt_profile,
        &[
            "First turn.",
            "Second turn.",
            "Third turn.",
            "Fourth turn to trigger depth prompt.",
        ],
    )
    .await
}

fn assert_request_matches_prompt_profile(
    request: &ResponsesRequest,
    prompt_profile: &PromptSource,
) {
    let instructions = request.instructions_text();
    let message_entries = request_message_entries(request);
    let all_message_text = message_entries
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let character_name = prompt_profile_name(prompt_profile)
        .unwrap_or("character")
        .to_string();

    assert!(
        instructions.contains("You are operating inside the Codex CLI runtime."),
        "expected runtime contract in instructions, got {instructions}"
    );
    assert!(
        instructions.contains("<active_card_prompt>"),
        "expected active card prompt block in instructions, got {instructions}"
    );
    assert!(
        !message_entries.iter().any(|(role, _)| role == "system"),
        "expected OpenAI Responses input to contain no system messages, got {message_entries:?}"
    );
    assert!(
        instructions.contains(&format!("Name: {character_name}")),
        "expected imported character name in instructions, got {instructions}"
    );
    assert!(
        !instructions
            .contains("Your default personality and tone is concise, direct, and friendly."),
        "expected default Codex persona to be replaced, got {instructions}"
    );

    if let Some(description) = prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.description.as_deref())
    {
        let rendered = render_prompt_profile_text(description, prompt_profile);
        assert!(
            instructions.contains(&format!("Description: {rendered}")),
            "expected rendered description in instructions, got {instructions}"
        );
    } else {
        assert!(
            !instructions.contains("\nDescription: "),
            "did not expect a description section, got {instructions}"
        );
    }

    if let Some(personality) = prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.personality.as_deref())
    {
        let rendered = render_prompt_profile_text(personality, prompt_profile);
        assert!(
            instructions.contains(&format!("Personality: {rendered}")),
            "expected rendered personality in instructions, got {instructions}"
        );
    } else {
        assert!(
            !instructions.contains("\nPersonality: "),
            "did not expect a personality section, got {instructions}"
        );
    }

    if let Some(scenario) = prompt_profile.scenario.as_deref() {
        let rendered = render_prompt_profile_text(scenario, prompt_profile);
        assert!(
            instructions.contains(&format!("Scenario: {rendered}")),
            "expected rendered scenario in instructions, got {instructions}"
        );
    } else {
        assert!(
            !instructions.contains("\nScenario: "),
            "did not expect a scenario section, got {instructions}"
        );
    }

    if let Some(creator_notes) = prompt_profile.creator_notes.as_deref() {
        let rendered = render_prompt_profile_text(creator_notes, prompt_profile);
        assert!(
            instructions.contains(&format!("Creator Notes: {rendered}")),
            "expected rendered creator notes in instructions, got {instructions}"
        );
    } else {
        assert!(
            !instructions.contains("\nCreator Notes: "),
            "did not expect creator notes in instructions, got {instructions}"
        );
    }

    if let Some(greeting) = prompt_profile_primary_greeting(prompt_profile) {
        let assistant_messages = role_texts(&message_entries, "assistant");
        assert!(
            assistant_messages.iter().any(|text| *text == greeting),
            "expected visible greeting to be present in assistant history, got {assistant_messages:?}"
        );
    }

    for example in &prompt_profile.examples {
        for message in &example.messages {
            let rendered = render_prompt_profile_text(&message.content, prompt_profile);
            let role = openai_responses_role(message.role);
            let role_messages = role_texts(&message_entries, role);
            assert!(
                role_messages.iter().any(|text| *text == rendered),
                "expected example message for role `{role}` to be present, got {role_messages:?}"
            );
        }
    }

    if let Some(post_history_instructions) = prompt_profile.post_history_instructions.as_deref() {
        let rendered = render_prompt_profile_text(post_history_instructions, prompt_profile);
        let developer_messages = role_texts(&message_entries, "developer");
        assert!(
            developer_messages.iter().any(|text| *text == rendered),
            "expected post-history instructions in developer messages, got {developer_messages:?}"
        );
    }

    if let Some(depth_prompt) = &prompt_profile.depth_prompt {
        let rendered = render_prompt_profile_text(&depth_prompt.content, prompt_profile);
        let role = openai_responses_role(depth_prompt.role);
        let role_messages = role_texts(&message_entries, role);
        assert!(
            role_messages.iter().any(|text| *text == rendered),
            "expected depth prompt for role `{role}` to be present, got {role_messages:?}"
        );
    }

    for constant_lore in prompt_profile_constant_lore_contents(prompt_profile) {
        assert!(
            instructions.contains(&constant_lore),
            "expected constant lore content in instructions, got {instructions}"
        );
    }

    for unresolved in ["{{user}}", "{{char}}", "{{original}}"] {
        assert!(
            !instructions.contains(unresolved),
            "did not expect unresolved placeholder `{unresolved}` in instructions, got {instructions}"
        );
        assert!(
            !all_message_text.contains(unresolved),
            "did not expect unresolved placeholder `{unresolved}` in request messages, got {all_message_text}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_start_persists_prompt_profile_in_session_meta() -> Result<()> {
    let server = MockServer::start().await;
    mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let test = test_codex().build(&server).await?;
    let prompt_profile = PromptSource {
        name: Some("Rei Kurose".to_string()),
        identity: Some(PromptIdentity {
            name: Some("Rei Kurose".to_string()),
            description: Some("A quiet late-night engineering companion.".to_string()),
            personality: Some("Restrained, observant, surgical.".to_string()),
        }),
        scenario: Some("Late-night pair debugging in quiet places.".to_string()),
        ..Default::default()
    };

    let new_thread = test
        .thread_manager
        .start_thread_with_tools(
            test.config.clone(),
            Vec::new(),
            Some(prompt_profile.clone()),
            false,
        )
        .await?;

    new_thread
        .thread
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "persist prompt profile".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: new_thread.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;
    let turn_id = wait_for_event_match(&new_thread.thread, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;
    wait_for_event(&new_thread.thread, |event| match event {
        EventMsg::TurnComplete(event) => event.turn_id == turn_id,
        _ => false,
    })
    .await;

    let rollout_path = new_thread
        .thread
        .rollout_path()
        .context("thread start should materialize a rollout path")?;
    let session_meta_line = loop {
        match read_session_meta_line(rollout_path.as_path()).await {
            Ok(line) => break line,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(err) => return Err(err.into()),
        }
    };

    assert_eq!(
        session_meta_line.meta.prompt_profile,
        Some(prompt_profile.clone())
    );
    assert_eq!(
        new_thread.thread.prompt_profile().await,
        Some(prompt_profile)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_profile_compiles_model_visible_order_for_sampling_request() -> Result<()> {
    let server = start_mock_server().await;
    let responses = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "review complete"),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2-codex")
        .build(&server)
        .await?;

    let prompt_profile = PromptSource {
        name: Some("Rei Kurose".to_string()),
        identity: Some(PromptIdentity {
            name: Some("Rei Kurose".to_string()),
            description: Some("A quiet late-night engineering companion.".to_string()),
            personality: Some("Restrained and surgical.".to_string()),
        }),
        scenario: Some("Late-night pair debugging.".to_string()),
        system_overlay: Some(
            "You are {{char}}.\n{{original}}\nTreat this as real engineering work.".to_string(),
        ),
        post_history_instructions: Some("Stay in character as {{char}}.".to_string()),
        greetings: vec![PromptGreeting {
            kind: PromptGreetingKind::Primary,
            text: "The carriage is quiet tonight. {{char}} is listening.".to_string(),
        }],
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

    let new_thread = test
        .thread_manager
        .start_thread_with_tools(test.config.clone(), Vec::new(), Some(prompt_profile), false)
        .await?;

    new_thread
        .thread
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "Review this parser.".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: new_thread.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;
    let turn_id = wait_for_event_match(&new_thread.thread, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;
    wait_for_event(&new_thread.thread, |event| match event {
        EventMsg::TurnComplete(event) => event.turn_id == turn_id,
        _ => false,
    })
    .await;

    let request = responses.single_request();
    let instructions = request.instructions_text();
    assert!(
        instructions.contains("You are operating inside the Codex CLI runtime."),
        "expected runtime contract in instructions, got {instructions}"
    );
    assert!(
        instructions.contains("<active_card_prompt>"),
        "expected active card prompt block in instructions, got {instructions}"
    );
    assert!(
        instructions.contains("You are Rei Kurose."),
        "expected prompt-profile overlay in instructions, got {instructions}"
    );
    assert!(
        instructions.contains("Treat this as real engineering work."),
        "expected prompt-profile overlay tail in instructions, got {instructions}"
    );
    assert!(
        !instructions
            .contains("Your default personality and tone is concise, direct, and friendly."),
        "expected default Codex persona to be replaced, got {instructions}"
    );

    assert_eq!(
        labeled_prompt_profile_order(&request),
        vec![
            "user:example_user",
            "assistant:example_assistant",
            "assistant:greeting",
            "developer:post_history",
            "developer:depth_prompt",
            "user:current_user",
        ]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fork_thread_clearing_prompt_profile_removes_inherited_profile() -> Result<()> {
    let server = MockServer::start().await;
    mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let test = test_codex().build(&server).await?;
    let prompt_profile = PromptSource {
        name: Some("Rei Kurose".to_string()),
        identity: Some(PromptIdentity {
            name: Some("Rei Kurose".to_string()),
            description: Some("A quiet late-night engineering companion.".to_string()),
            personality: Some("Restrained, observant, surgical.".to_string()),
        }),
        ..Default::default()
    };

    let new_thread = test
        .thread_manager
        .start_thread_with_tools(
            test.config.clone(),
            Vec::new(),
            Some(prompt_profile.clone()),
            false,
        )
        .await?;

    new_thread
        .thread
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "materialize rollout".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: new_thread.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;
    let turn_id = wait_for_event_match(&new_thread.thread, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;
    wait_for_event(&new_thread.thread, |event| match event {
        EventMsg::TurnComplete(event) => event.turn_id == turn_id,
        _ => false,
    })
    .await;

    let rollout_path = new_thread
        .thread
        .rollout_path()
        .context("thread should materialize a rollout path before fork")?;
    let forked = test
        .thread_manager
        .fork_thread_clearing_prompt_profile(usize::MAX, test.config.clone(), rollout_path, false)
        .await?;

    assert_eq!(forked.thread.prompt_profile().await, None);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixture_xie_zhilin_card_compiles_expected_model_visible_layout() -> Result<()> {
    let prompt_profile = load_bundled_xie_zhilin_prompt_profile()?;
    let request = capture_fourth_request_for_prompt_profile(prompt_profile.clone()).await?;
    let instructions = request.instructions_text();
    let message_entries = request_message_entries(&request);
    let assistant_messages = role_texts(&message_entries, "assistant");
    let user_messages = role_texts(&message_entries, "user");
    let system_messages = role_texts(&message_entries, "system");
    let developer_messages = role_texts(&message_entries, "developer");
    let primary_greeting =
        prompt_profile_primary_greeting(&prompt_profile).context("primary greeting")?;
    let alternate_greetings = prompt_profile
        .greetings
        .iter()
        .filter(|greeting| greeting.kind == PromptGreetingKind::Alternate)
        .map(|greeting| render_prompt_profile_text(&greeting.text, &prompt_profile))
        .collect::<Vec<_>>();
    let all_texts = all_message_texts(&message_entries);

    assert_request_matches_prompt_profile(&request, &prompt_profile);

    assert!(
        instructions.contains("Description: # 系统指令"),
        "expected the long description to drive the active card prompt, got {instructions}"
    );
    assert!(
        instructions.contains("以下是谢知凛（即谢知凛）的性方面描述"),
        "expected before_char lore from the bundled PNG in instructions, got {instructions}"
    );
    assert!(
        !instructions.contains("\nPersonality: "),
        "did not expect an empty personality field to render, got {instructions}"
    );
    assert!(
        !instructions.contains("\nScenario: "),
        "did not expect an empty scenario field to render, got {instructions}"
    );
    assert!(
        !instructions.contains("\nCreator Notes: "),
        "did not expect empty creator notes to render, got {instructions}"
    );

    assert!(
        system_messages.is_empty(),
        "expected OpenAI Responses request to contain no system messages, got {system_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("Write 谢知凛's next reply in a fictional chat")),
        "expected ST narrative instruction in developer messages, got {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("<roleplay_rules>")),
        "expected late roleplay rules injection in developer messages, got {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("<任务内容要求>")),
        "expected late task requirements injection in developer messages, got {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("<story_tone>")),
        "expected late story tone injection in developer messages, got {developer_messages:?}"
    );
    assert_eq!(
        &assistant_messages[..4],
        &[
            primary_greeting.as_str(),
            "turn 1 complete",
            "turn 2 complete",
            "turn 3 complete"
        ],
    );
    assert!(
        assistant_messages.last().is_some_and(|text| {
            text.contains("<system_constraints>")
                && text.contains("<status_bar>")
                && text.contains("<system_bar>")
        }),
        "expected assistant prefill with system/status bar guidance, got {assistant_messages:?}"
    );
    assert_eq!(
        &user_messages[user_messages.len().saturating_sub(4)..],
        &[
            "First turn.",
            "Second turn.",
            "Third turn.",
            "Fourth turn to trigger depth prompt.",
        ],
    );

    for alternate_greeting in alternate_greetings {
        assert!(
            !assistant_messages.contains(&alternate_greeting.as_str()),
            "did not expect alternate greeting to be auto-injected, got {assistant_messages:?}"
        );
    }

    assert!(
        developer_messages.len() >= 2,
        "expected harness developer messages to remain present, got {developer_messages:?}"
    );

    for lore_only_text in prompt_profile_constant_lore_contents(&prompt_profile) {
        assert!(
            instructions.contains(&lore_only_text),
            "expected constant lorebook content in instructions, got {instructions}"
        );
    }

    assert!(
        !all_texts
            .iter()
            .any(|text| text.contains("<world_setting>")),
        "did not expect world-setting lore to leak into request messages, got {all_texts:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyword_lore_entries_inject_into_active_card_prompt() -> Result<()> {
    let prompt_profile = PromptSource {
        name: Some("Lore Weaver".to_string()),
        identity: Some(PromptIdentity {
            name: Some("Lore Weaver".to_string()),
            description: Some("A librarian who notices every hint.".to_string()),
            personality: None,
        }),
        knowledge: vec![codex_protocol::prompt_profile::PromptKnowledgeSource {
            name: Some("Lore".to_string()),
            kind: Some("characterBook".to_string()),
            description: None,
            entries: vec![
                codex_protocol::prompt_profile::PromptKnowledgeEntry {
                    id: Some("before".to_string()),
                    keys: Vec::new(),
                    secondary_keys: Vec::new(),
                    content: "<always_on>Always scan the room first.</always_on>".to_string(),
                    enabled: true,
                    insertion_order: Some(1),
                    position: Some("before_char".to_string()),
                    metadata: Some(serde_json::json!({
                        "constant": true,
                        "use_regex": true
                    })),
                },
                codex_protocol::prompt_profile::PromptKnowledgeEntry {
                    id: Some("after".to_string()),
                    keys: vec!["par.*ser".to_string()],
                    secondary_keys: Vec::new(),
                    content:
                        "<parser_lore>Focus on the first divergent parser state.</parser_lore>"
                            .to_string(),
                    enabled: true,
                    insertion_order: Some(2),
                    position: Some("after_char".to_string()),
                    metadata: Some(serde_json::json!({
                        "constant": false,
                        "use_regex": true
                    })),
                },
            ],
            metadata: None,
        }],
        ..Default::default()
    };

    let request = capture_last_request_for_prompt_profile_turns(
        prompt_profile,
        &["Review this parser before touching the code."],
    )
    .await?;
    let instructions = request.instructions_text();
    let before_index = instructions
        .find("<always_on>Always scan the room first.</always_on>")
        .context("before_char lore")?;
    let name_index = instructions
        .find("Name: Lore Weaver")
        .context("profile name")?;
    let after_index = instructions
        .find("<parser_lore>Focus on the first divergent parser state.</parser_lore>")
        .context("after_char lore")?;

    assert!(
        before_index < name_index,
        "expected before_char lore before the character body, got {instructions}"
    );
    assert!(
        after_index > name_index,
        "expected after_char lore after the character body, got {instructions}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_xie_zhilin_card_imports_real_png_settings() -> Result<()> {
    let prompt_profile = load_bundled_xie_zhilin_prompt_profile()?;
    let world_book_entries = prompt_profile_world_book_entries(&prompt_profile);

    assert_eq!(prompt_profile.system_overlay, None);
    assert_eq!(prompt_profile.post_history_instructions, None);
    assert_eq!(prompt_profile.examples, Vec::new());
    assert_eq!(prompt_profile.depth_prompt, None);
    assert_eq!(prompt_profile.greetings.len(), 5);
    assert!(
        prompt_profile
            .knowledge
            .iter()
            .any(|source| source.kind.as_deref() == Some("worldRef")),
        "expected linked world reference to be preserved"
    );
    assert!(
        prompt_profile
            .knowledge
            .iter()
            .any(|source| source.kind.as_deref() == Some("worldBook")),
        "expected linked world to be imported as a renderable worldBook"
    );
    assert_eq!(world_book_entries.len(), 10);

    let character_book = prompt_profile
        .knowledge
        .iter()
        .find(|source| source.kind.as_deref() == Some("characterBook"))
        .context("expected real Xie card to contain a character book")?;
    assert_eq!(character_book.name.as_deref(), Some("18🚫系统"));
    assert_eq!(character_book.entries.len(), 10);

    let constant_count = character_book
        .entries
        .iter()
        .filter(|entry| metadata_bool(entry.metadata.as_ref(), "constant"))
        .count();
    let regex_count = character_book
        .entries
        .iter()
        .filter(|entry| metadata_bool(entry.metadata.as_ref(), "use_regex"))
        .count();
    assert_eq!(constant_count, 8);
    assert_eq!(regex_count, 10);
    assert!(
        character_book
            .entries
            .iter()
            .any(|entry| entry.position.as_deref() == Some("before_char"))
    );
    assert!(
        character_book
            .entries
            .iter()
            .any(|entry| entry.position.as_deref() == Some("after_char"))
    );
    assert!(
        character_book
            .entries
            .iter()
            .any(|entry| !metadata_bool(entry.metadata.as_ref(), "constant"))
    );
    assert!(
        world_book_entries.iter().any(|entry| {
            metadata_nested_i64(entry.metadata.as_ref(), "extensions", "position") == Some(4)
                && metadata_nested_i64(entry.metadata.as_ref(), "extensions", "role") == Some(2)
        }),
        "expected worldBook to preserve assistant-prefill metadata"
    );

    let raw_extensions = prompt_profile
        .raw_extensions
        .as_ref()
        .and_then(JsonValue::as_object)
        .context("expected raw extensions to be preserved")?;
    assert!(raw_extensions.contains_key("talkativeness"));
    assert!(raw_extensions.contains_key("fav"));
    assert!(raw_extensions.contains_key("world"));
    assert!(raw_extensions.contains_key("depth_prompt"));
    assert!(raw_extensions.contains_key("regex_scripts"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_xie_zhilin_card_keyword_lore_triggers_into_active_card_prompt() -> Result<()> {
    let prompt_profile = load_bundled_xie_zhilin_prompt_profile()?;

    let request = capture_last_request_for_prompt_profile_turns(
        prompt_profile.clone(),
        &["沈清澜刚在篮球队经理那边提醒我，运动会报名今天截止。"],
    )
    .await?;
    let instructions = request.instructions_text();
    let message_entries = request_message_entries(&request);
    let all_texts = all_message_texts(&message_entries);
    let name_index = instructions
        .find("Name: 谢知凛")
        .context("expected Xie card name in instructions")?;
    let character_book_entries = prompt_profile_character_book_entries(&prompt_profile);
    let before_char_entry = character_book_entries
        .iter()
        .copied()
        .find(|entry| {
            entry.position.as_deref() == Some("before_char")
                && metadata_bool(entry.metadata.as_ref(), "constant")
        })
        .context("expected before_char constant lore")?;
    let npc_entry = character_book_entries
        .iter()
        .copied()
        .find(|entry| entry.keys.iter().any(|key| key == "沈清澜"))
        .context("expected NPC lore entry")?;
    let activity_entry = character_book_entries
        .iter()
        .copied()
        .find(|entry| entry.keys.iter().any(|key| key == "运动会"))
        .context("expected activity lore entry")?;

    let before_char_rendered =
        render_prompt_profile_text(&before_char_entry.content, &prompt_profile);
    let npc_rendered = render_prompt_profile_text(&npc_entry.content, &prompt_profile);
    let activity_rendered = render_prompt_profile_text(&activity_entry.content, &prompt_profile);
    let before_char_index = instructions
        .find(&before_char_rendered)
        .context("expected before_char lore content in instructions")?;
    let npc_index = instructions
        .find(&npc_rendered)
        .context("expected NPC lore content in instructions")?;
    let activity_index = instructions
        .find(&activity_rendered)
        .context("expected activity lore content in instructions")?;

    assert!(
        before_char_index < name_index,
        "expected before_char lore before the active card body, got {instructions}"
    );
    assert!(
        npc_index > name_index,
        "expected NPC lore after the active card body, got {instructions}"
    );
    assert!(
        activity_index > name_index,
        "expected activity lore after the active card body, got {instructions}"
    );
    assert!(
        !all_texts
            .iter()
            .any(|text| text.contains("班级学习委员 / 校篮球队经理")),
        "did not expect NPC lore to leak into visible messages, got {all_texts:?}"
    );
    assert!(
        !all_texts
            .iter()
            .any(|text| text.contains("春季运动会（通常四月中下旬）")),
        "did not expect activity lore to leak into visible messages, got {all_texts:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xie_zhilin_tool_continuation_keeps_roleplay_tail_and_assistant_prefix() -> Result<()> {
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    "plan-tool-call",
                    "update_plan",
                    &serde_json::json!({
                        "explanation": "Inspect file",
                        "plan": [
                            {"step": "Inspect file", "status": "in_progress"},
                            {"step": "Reply in character", "status": "pending"},
                        ],
                    })
                    .to_string(),
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-2", "in character"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model("gpt-5.2-codex")
        .build(&server)
        .await?;
    let prompt_profile = load_bundled_xie_zhilin_prompt_profile()?;
    let new_thread = test
        .thread_manager
        .start_thread_with_tools(
            test.config.clone(),
            Vec::new(),
            Some(prompt_profile.clone()),
            false,
        )
        .await?;

    new_thread
        .thread
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "先帮我看一下这个文件，然后告诉我是什么。".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.cwd_path().to_path_buf(),
            approval_policy: codex_protocol::protocol::AskForApproval::Never,
            sandbox_policy: SandboxPolicy::DangerFullAccess,
            model: new_thread.session_configured.model.clone(),
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;
    let turn_id = wait_for_event_match(&new_thread.thread, |event| match event {
        EventMsg::TurnStarted(event) => Some(event.turn_id.clone()),
        _ => None,
    })
    .await;
    wait_for_event(&new_thread.thread, |event| match event {
        EventMsg::TurnComplete(event) => event.turn_id == turn_id,
        _ => false,
    })
    .await;

    let request = responses
        .last_request()
        .context("expected second model request after tool output")?;
    let message_entries = request_message_entries(&request);
    let developer_messages = role_texts(&message_entries, "developer");
    let last_message = message_entries
        .last()
        .context("expected at least one message in tool continuation request")?;
    let labeled_input = request
        .input()
        .into_iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call_output") => Some("function_call_output".to_string()),
            Some("message") => {
                let role = item.get("role").and_then(Value::as_str)?;
                let text = item
                    .get("content")
                    .and_then(Value::as_array)?
                    .iter()
                    .filter_map(|content| content.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.contains("The next assistant message is a continuation prefix") {
                    Some(format!("{role}:post_tool_roleplay"))
                } else if text.contains("<system_constraints>")
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

    assert_request_matches_prompt_profile(&request, &prompt_profile);
    assert!(
        developer_messages.iter().any(|text| {
            text.contains("Keep responding as 谢知凛.")
                && text.contains("The next assistant message is a continuation prefix")
        }),
        "expected hard post-tool roleplay reminder in developer messages, got {developer_messages:?}"
    );
    assert_eq!(
        labeled_input,
        vec![
            "function_call_output".to_string(),
            "developer:post_tool_roleplay".to_string(),
            "assistant:assistant_prefix".to_string(),
        ]
    );
    assert_eq!(last_message.0, "assistant");
    assert!(
        last_message.1.contains("<system_constraints>")
            && last_message.1.contains("<status_bar>")
            && last_message.1.contains("<system_bar>"),
        "expected final message in tool continuation request to be the assistant prefix, got {last_message:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_sillytavern_card_compiles_visible_and_hidden_messages() -> Result<()> {
    let card_path = match std::env::var_os("CODEX_TEST_PROMPT_PROFILE_CARD") {
        Some(path) => std::path::PathBuf::from(path),
        None => bundled_xie_zhilin_png_fixture()?,
    };
    let prompt_profile = load_prompt_profile_from_path(&card_path)?;
    let character_name = prompt_profile_name(&prompt_profile).unwrap_or("character");
    let request = capture_fourth_request_for_prompt_profile(prompt_profile.clone()).await?;
    let instructions = request.instructions_text();
    let snapshot = context_snapshot::format_request_input_snapshot(
        &request,
        &ContextSnapshotOptions::default().render_mode(ContextSnapshotRenderMode::FullText),
    );
    println!(
        "External prompt-profile card: {}\nCharacter: {character_name}\n\nInstructions:\n{instructions}\n\nInput:\n{snapshot}",
        card_path.display()
    );

    assert_request_matches_prompt_profile(&request, &prompt_profile);

    Ok(())
}
