#![allow(clippy::unwrap_used)]

use anyhow::Result;
use codex_core::StartIfIdleSubmission;
use codex_core::StartThreadOptions;
use codex_core::ThreadManager;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_core::config::Config;
use codex_core::thread_store_from_config;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::ExecutorFileSystem;
use codex_extension_api::empty_extension_registry;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::SkillInvocationContributor;
use codex_extension_api::SkillInvocationInput;
use codex_extension_api::SkillInvocationKind;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_login::CodexAuth;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use codex_skills_extension::SkillsExtensionConfig;
use codex_skills_extension::install;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use core_test_support::create_directory_symlink;
use core_test_support::load_default_config_for_test;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_remote;
use core_test_support::skip_if_wine_exec;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tempfile::TempDir;

#[derive(Default)]
struct SkillInvocationRecorder(Mutex<Vec<(String, SkillInvocationKind)>>);

impl SkillInvocationContributor for SkillInvocationRecorder {
    fn on_skill_invocation<'a>(
        &'a self,
        input: SkillInvocationInput<'a>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((input.skill_resource.to_owned(), input.kind));
        })
    }
}

async fn write_repo_skill(
    cwd: AbsolutePathBuf,
    fs: Arc<dyn ExecutorFileSystem>,
    name: &str,
    description: &str,
    body: &str,
) -> Result<()> {
    let skill_dir = cwd.join(".agents").join("skills").join(name);
    let skill_dir_uri = PathUri::from_host_native_path(&skill_dir)?;
    fs.create_directory(
        &skill_dir_uri,
        CreateDirectoryOptions {
            recursive: true,
            follow_symlinks: true,
        },
        /*sandbox*/ None,
    )
    .await?;
    let contents = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
    let path = skill_dir.join("SKILL.md");
    let path_uri = PathUri::from_host_native_path(&path)?;
    fs.write_file(
        &path_uri,
        contents.into_bytes(),
        Default::default(),
        /*sandbox*/ None,
    )
    .await?;
    Ok(())
}

fn write_home_skill(codex_home: &Path, dir: &str, name: &str, description: &str) -> Result<()> {
    let skill_dir = codex_home.join("skills").join(dir);
    fs::create_dir_all(&skill_dir)?;
    let contents = format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n");
    fs::write(skill_dir.join("SKILL.md"), contents)?;
    Ok(())
}

fn system_skill_md_path(codex_home: &Path, system_skill_name: &str) -> PathBuf {
    codex_home
        .join("skills")
        .join(".system")
        .join(system_skill_name)
        .join("SKILL.md")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_turn_includes_skill_instructions() -> Result<()> {
    skip_if_wine_exec!(
        Ok(()),
        "skill paths require matching host and executor path conventions"
    );
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let skill_body = "skill body";
    let recorder = Arc::new(SkillInvocationRecorder::default());
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.skill_invocation_contributor(recorder.clone());
    let mut builder = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_workspace_setup(move |cwd, fs| async move {
            write_repo_skill(cwd, fs, "demo", "demo skill", skill_body).await
        });
    let test = builder.build_with_auto_env(&server).await?;

    let skill_path = test
        .config
        .cwd
        .join(".agents/skills/demo/SKILL.md")
        .canonicalize()
        .unwrap_or_else(|_| test.config.cwd.join(".agents/skills/demo/SKILL.md"))
        .to_path_buf();

    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let session_model = test.session_configured.model.clone();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![
                UserInput::Text {
                    text: "please use $demo".to_string(),
                    text_elements: Vec::new(),
                },
                UserInput::Skill {
                    name: "demo".to_string(),
                    path: skill_path.clone(),
                },
            ])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    core_test_support::wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let request = mock.single_request();
    let user_texts = request.message_input_texts("user");
    let skill_path_str = skill_path.to_string_lossy();
    assert!(
        user_texts.iter().any(|text| {
            text.contains("<skill>\n<name>demo</name>")
                && text.contains("<path>")
                && text.contains(skill_body)
                && text.contains(skill_path_str.as_ref())
        }),
        "expected skill instructions in user input, got {user_texts:?}"
    );
    assert!(request.has_content_kinds(&["skills.selected_skill_instructions"]));
    assert_eq!(
        *recorder
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![(
            skill_path.display().to_string(),
            SkillInvocationKind::Explicit
        )],
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_turn_selects_symlinked_skill_by_advertised_discovery_path() -> Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_remote!(
        Ok(()),
        "remote filesystems do not expose directory symlink creation"
    );

    let server = start_mock_server().await;
    let skill_body = "instructions from the canonical linked skill";
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    install(&mut extensions, |config: &Config| SkillsExtensionConfig {
        include_instructions: config.include_skill_instructions,
        max_context_tokens: config.skill_max_context_tokens,
        bundled_skills_enabled: false,
        orchestrator_skills_enabled: false,
        shadow_selection_enabled: false,
    });
    let mut builder = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_workspace_setup(move |cwd, _fs| async move {
            let source_skill_dir = cwd.join("shared-skills/linked-demo");
            let discovery_root = cwd.join(".agents/skills");
            std::fs::create_dir_all(source_skill_dir.as_path())?;
            std::fs::create_dir_all(discovery_root.as_path())?;
            std::fs::write(
                source_skill_dir.join("SKILL.md"),
                format!(
                    "---\nname: linked-demo\ndescription: Linked demo skill\n---\n\n{skill_body}\n"
                ),
            )?;
            create_directory_symlink(
                source_skill_dir.as_path(),
                discovery_root.join("linked-demo").as_path(),
            );
            Ok(())
        });
    let test = builder.build_with_auto_env(&server).await?;
    let discovery_root = test.config.cwd.join(".agents/skills").canonicalize()?;
    let discovery_path = discovery_root.join("linked-demo/SKILL.md");
    let canonical_path = discovery_path.canonicalize()?;
    let discovery_path_display = discovery_path.display();
    let canonical_path_display = canonical_path.display();
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("linked-skill-response"),
            ev_assistant_message("linked-skill-message", "done"),
            ev_completed("linked-skill-response"),
        ]),
    )
    .await;

    let submission = test
        .codex
        .start_turn_if_idle(TurnInputRequest::new(TurnInput::UserInput {
            content: vec![
                UserInput::Text {
                    text: format!("please use [$linked-demo]({discovery_path_display})"),
                    text_elements: Vec::new(),
                },
                UserInput::Skill {
                    name: "linked-demo".to_string(),
                    path: discovery_path.to_path_buf(),
                },
            ],
            client_id: Some("linked-skill-user-message".to_string()),
        }))
        .await?;
    assert!(matches!(submission, StartIfIdleSubmission::Started { .. }));

    core_test_support::wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let request = mock.single_request();
    let developer_texts = request.message_input_texts("developer");
    let discovery_root_display = discovery_root.to_string_lossy().replace('\\', "/");
    let root_suffix = format!(" = `{discovery_root_display}`");
    let discovery_root_alias = developer_texts
        .iter()
        .flat_map(|text| text.lines())
        .find(|line| line.ends_with(&root_suffix))
        .and_then(|line| line.strip_prefix("- `"))
        .and_then(|line| line.split_once("` = ").map(|(alias, _)| alias))
        .expect("skill catalog should alias the advertised discovery root");
    let advertised_path = format!("(file: {discovery_root_alias}/linked-demo/SKILL.md)");
    assert!(
        developer_texts
            .iter()
            .any(|text| text.contains(&advertised_path)),
        "expected symlink discovery path in the skill catalog, got {developer_texts:?}"
    );

    let user_texts = request.message_input_texts("user");
    let canonical_identity = format!("<path>{canonical_path_display}</path>");
    assert!(
        user_texts.iter().any(|text| {
            text.contains("<skill>\n<name>linked-demo</name>")
                && text.contains(&canonical_identity)
                && text.contains(skill_body)
        }),
        "expected canonical skill instructions selected by discovery path, got {user_texts:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_user_turn_includes_skill_instructions_in_the_first_request() -> Result<()> {
    skip_if_wine_exec!(
        Ok(()),
        "skill paths require matching host and executor path conventions"
    );
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let skill_body = "queued skill body";
    let mut builder = test_codex().with_workspace_setup(move |cwd, fs| async move {
        write_repo_skill(cwd, fs, "queued-demo", "queued demo skill", skill_body).await
    });
    let test = builder.build_with_auto_env(&server).await?;
    let skill_path = test
        .config
        .cwd
        .join(".agents/skills/queued-demo/SKILL.md")
        .canonicalize()
        .unwrap_or_else(|_| test.config.cwd.join(".agents/skills/queued-demo/SKILL.md"))
        .to_path_buf();
    let mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("queued-skill-response"),
            ev_assistant_message("queued-skill-message", "done"),
            ev_completed("queued-skill-response"),
        ]),
    )
    .await;

    let submission = test
        .codex
        .start_turn_if_idle(TurnInputRequest::new(TurnInput::UserInput {
            content: vec![
                UserInput::Text {
                    text: "please use $queued-demo".to_string(),
                    text_elements: Vec::new(),
                },
                UserInput::Skill {
                    name: "queued-demo".to_string(),
                    path: skill_path.clone(),
                },
            ],
            client_id: Some("queued-skill-user-message".to_string()),
        }))
        .await?;
    assert!(matches!(submission, StartIfIdleSubmission::Started { .. }));

    core_test_support::wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;

    let user_texts = mock.single_request().message_input_texts("user");
    let skill_path_str = skill_path.to_string_lossy();
    assert!(
        user_texts.iter().any(|text| {
            text.contains("<skill>\n<name>queued-demo</name>")
                && text.contains("<path>")
                && text.contains(skill_body)
                && text.contains(skill_path_str.as_ref())
        }),
        "expected queued skill instructions in the first request, got {user_texts:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(any())]
async fn list_skills_includes_repo_and_home_skills_remote_aware() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex()
        .with_pre_build_hook(|home| {
            write_home_skill(home, "home-demo", "home-demo", "from home")
                .expect("write home skill");
        })
        .with_workspace_setup(|cwd, fs| async move {
            write_repo_skill(cwd, fs, "repo-demo", "from repo", "# Body").await
        });
    let test = builder.build_remote_aware(&server).await?;

    test.codex
        .submit(Op::ListSkills {
            cwds: Vec::new(),
            force_reload: true,
        })
        .await?;
    let response =
        core_test_support::wait_for_event_match(test.codex.as_ref(), |event| match event {
            codex_protocol::protocol::EventMsg::ListSkillsResponse(response) => {
                Some(response.clone())
            }
            _ => None,
        })
        .await;

    let cwd = test.config.cwd.as_path();
    let skills = response
        .skills
        .iter()
        .find(|entry| entry.cwd.as_path() == cwd)
        .map(|entry| entry.skills.clone())
        .unwrap_or_default();

    let repo_skill = skills
        .iter()
        .find(|skill| skill.name == "repo-demo")
        .expect("expected repo skill");
    assert_eq!(repo_skill.scope, codex_protocol::protocol::SkillScope::Repo);
    let repo_path = repo_skill.path.to_string_lossy().replace('\\', "/");
    assert!(
        repo_path.ends_with("/.agents/skills/repo-demo/SKILL.md"),
        "unexpected repo skill path: {repo_path}"
    );

    let home_skill = skills
        .iter()
        .find(|skill| skill.name == "home-demo")
        .expect("expected home skill");
    assert_eq!(home_skill.scope, codex_protocol::protocol::SkillScope::User);
    let home_path = home_skill.path.to_string_lossy().replace('\\', "/");
    assert!(
        home_path.ends_with("/skills/home-demo/SKILL.md"),
        "unexpected home skill path: {home_path}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(any())]
async fn list_skills_skips_cwd_roots_when_environment_disabled() -> Result<()> {
    let codex_home = TempDir::new()?;
    let cwd = TempDir::new()?;
    write_home_skill(
        codex_home.path(),
        "home-disabled",
        "home-disabled",
        "from home",
    )?;
    let repo_skill_dir = cwd
        .path()
        .join(".agents")
        .join("skills")
        .join("repo-disabled");
    fs::create_dir_all(&repo_skill_dir)?;
    fs::write(
        repo_skill_dir.join("SKILL.md"),
        "---\nname: repo-disabled\ndescription: from repo\n---\n\n# Body\n",
    )?;
    let mut config = load_default_config_for_test(&codex_home).await;
    config.cwd = AbsolutePathBuf::from_absolute_path_checked(cwd.path())?;

    let auth_manager =
        codex_core::test_support::auth_manager_from_auth(CodexAuth::from_api_key("dummy"));
    let installation_id = codex_core::resolve_installation_id(&config.codex_home).await?;
    let thread_manager = ThreadManager::new(
        &config,
        auth_manager.clone(),
        codex_core::build_models_manager(&config, auth_manager),
        codex_core::CodexAppsToolsCache::default(),
        SessionSource::Exec,
        Arc::new(codex_exec_server::EnvironmentManager::without_environments(
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        )),
        empty_extension_registry(),
        Arc::new(codex_core::test_support::EmptyUserInstructionsProvider),
        /*analytics_events_client*/ None,
        thread_store_from_config(&config, /*state_db*/ None),
        /*agent_graph_store*/ None,
        installation_id,
        /*attestation_provider*/ None,
        /*external_time_provider*/ None,
    );
    let new_thread = thread_manager
        .start_thread(StartThreadOptions::new(config.clone()))
        .await?;
    let cwd = config.cwd.to_path_buf();

    new_thread
        .thread
        .submit(Op::ListSkills {
            cwds: vec![cwd.clone()],
            force_reload: true,
        })
        .await?;
    let response =
        core_test_support::wait_for_event_match(new_thread.thread.as_ref(), |event| match event {
            codex_protocol::protocol::EventMsg::ListSkillsResponse(response) => {
                Some(response.clone())
            }
            _ => None,
        })
        .await;

    assert_eq!(response.skills.len(), 1);
    assert_eq!(response.skills[0].cwd, cwd);
    assert_eq!(response.skills[0].errors.len(), 0);
    assert!(
        response.skills[0]
            .skills
            .iter()
            .any(|skill| skill.name == "home-disabled")
    );
    assert!(
        response.skills[0]
            .skills
            .iter()
            .all(|skill| skill.name != "repo-disabled")
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(any())]
async fn skill_load_errors_surface_in_session_configured() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let mut builder = test_codex().with_pre_build_hook(|home| {
        let skill_dir = home.join("skills").join("broken");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "not yaml").unwrap();
    });
    let test = builder.build(&server).await?;

    test.codex
        .submit(Op::ListSkills {
            cwds: Vec::new(),
            force_reload: false,
        })
        .await?;
    let response =
        core_test_support::wait_for_event_match(test.codex.as_ref(), |event| match event {
            codex_protocol::protocol::EventMsg::ListSkillsResponse(response) => {
                Some(response.clone())
            }
            _ => None,
        })
        .await;

    let cwd = test.cwd_path();
    let (skills, errors) = response
        .skills
        .iter()
        .find(|entry| entry.cwd.as_path() == cwd)
        .map(|entry| (entry.skills.clone(), entry.errors.clone()))
        .unwrap_or_default();

    assert!(
        skills.iter().all(|skill| {
            !skill
                .path
                .to_string_lossy()
                .ends_with("skills/broken/SKILL.md")
        }),
        "expected broken skill not loaded, got {skills:?}"
    );
    assert_eq!(errors.len(), 1, "expected one load error");
    let error_path = errors[0].path.to_string_lossy();
    assert!(
        error_path.ends_with("skills/broken/SKILL.md"),
        "unexpected error path: {error_path}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(any())]
async fn list_skills_includes_system_cache_entries() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const SYSTEM_SKILL_NAMES: [&str; 2] = ["skill-creator", "dawn-im-management"];

    let server = start_mock_server().await;
    let mut builder = test_codex().with_pre_build_hook(|home| {
        for system_skill_name in SYSTEM_SKILL_NAMES {
            let system_skill_path = system_skill_md_path(home, system_skill_name);
            assert!(
                !system_skill_path.exists(),
                "expected embedded system skills not yet installed, but {system_skill_path:?} exists"
            );
        }
    });
    let test = builder.build(&server).await?;

    for system_skill_name in SYSTEM_SKILL_NAMES {
        let system_skill_path = system_skill_md_path(test.codex_home_path(), system_skill_name);
        assert!(
            system_skill_path.exists(),
            "expected embedded system skills installed to {system_skill_path:?}"
        );
        let system_skill_contents = fs::read_to_string(&system_skill_path)?;
        let expected_name_line = format!("name: {system_skill_name}");
        assert!(
            system_skill_contents.contains(&expected_name_line),
            "expected embedded system skill file, got:\n{system_skill_contents}"
        );
    }

    test.codex
        .submit(Op::ListSkills {
            cwds: Vec::new(),
            force_reload: true,
        })
        .await?;
    let response =
        core_test_support::wait_for_event_match(test.codex.as_ref(), |event| match event {
            codex_protocol::protocol::EventMsg::ListSkillsResponse(response) => {
                Some(response.clone())
            }
            _ => None,
        })
        .await;

    let cwd = test.cwd_path();
    let (skills, _errors) = response
        .skills
        .iter()
        .find(|entry| entry.cwd.as_path() == cwd)
        .map(|entry| (entry.skills.clone(), entry.errors.clone()))
        .unwrap_or_default();

    for system_skill_name in SYSTEM_SKILL_NAMES {
        let skill = skills
            .iter()
            .find(|skill| skill.name == system_skill_name)
            .unwrap_or_else(|| panic!("expected system skill '{system_skill_name}' to be present"));
        assert_eq!(skill.scope, codex_protocol::protocol::SkillScope::System);
        let path_str = skill.path.to_string_lossy().replace('\\', "/");
        let expected_path_suffix = format!("/skills/.system/{system_skill_name}/SKILL.md");
        assert!(
            path_str.ends_with(&expected_path_suffix),
            "unexpected skill path: {path_str}"
        );
    }

    Ok(())
}
