use std::path::PathBuf;
use std::time::Duration;

use codex_core::load_prompt_profile_from_path;
use codex_protocol::prompt_profile::PromptSource;
use codex_utils_absolute_path::AbsolutePathBuf;
use color_eyre::eyre::Result;
use ratatui::style::Stylize;
use ratatui::text::Line;

use super::App;
use super::AppRunControl;
use super::session_summary;

fn prompt_profile_display_name(prompt_profile: &PromptSource) -> String {
    prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.name.clone())
        .or_else(|| prompt_profile.name.clone())
        .unwrap_or_else(|| "Unnamed prompt profile".to_string())
}

pub(super) fn prompt_profile_summary_lines(prompt_profile: &PromptSource) -> Vec<Line<'static>> {
    let name = prompt_profile_display_name(prompt_profile);
    let mut lines = vec![
        vec!["• ".into(), "Active prompt profile: ".into(), name.cyan()].into(),
        vec![
            "  ".into(),
            format!(
                "Greetings: {} | Examples: {} | Knowledge sources: {}",
                prompt_profile.greetings.len(),
                prompt_profile.examples.len(),
                prompt_profile.knowledge.len()
            )
            .dim(),
        ]
        .into(),
    ];

    let mut active_layers = Vec::new();
    if prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.description.as_ref())
        .is_some()
    {
        active_layers.push("description");
    }
    if prompt_profile
        .identity
        .as_ref()
        .and_then(|identity| identity.personality.as_ref())
        .is_some()
    {
        active_layers.push("personality");
    }
    if prompt_profile.scenario.is_some() {
        active_layers.push("scenario");
    }
    if prompt_profile.system_overlay.is_some() {
        active_layers.push("system overlay");
    }
    if prompt_profile.post_history_instructions.is_some() {
        active_layers.push("post-history");
    }
    if prompt_profile.depth_prompt.is_some() {
        active_layers.push("depth prompt");
    }
    if !active_layers.is_empty() {
        lines.push(
            vec![
                "  ".into(),
                "Layers: ".dim(),
                active_layers.join(", ").dim(),
            ]
            .into(),
        );
    }

    if let Some(origin) = &prompt_profile.origin
        && (origin.format.is_some() || origin.source_path.is_some())
    {
        let mut spans = vec!["  ".into(), "Origin: ".dim()];
        if let Some(format) = origin.format.as_deref() {
            spans.push(format.to_string().dim());
        }
        if let Some(source_path) = origin.source_path.as_deref() {
            if origin.format.is_some() {
                spans.push(" ".dim());
            }
            spans.push(source_path.to_string().dim());
        }
        lines.push(spans.into());
    }

    lines
}

impl App {
    pub(super) async fn handle_load_prompt_profile(
        &mut self,
        tui: &mut crate::tui::Tui,
        path: PathBuf,
    ) -> Result<AppRunControl> {
        let resolved_path = match AbsolutePathBuf::resolve_path_against_base(path, &self.config.cwd)
        {
            Ok(path) => path.into_path_buf(),
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to resolve prompt profile path: {err}"));
                return Ok(AppRunControl::Continue);
            }
        };
        let prompt_profile = match load_prompt_profile_from_path(resolved_path.as_path()) {
            Ok(prompt_profile) => prompt_profile,
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to load prompt profile from {}: {err}",
                    resolved_path.display()
                ));
                return Ok(AppRunControl::Continue);
            }
        };
        let prompt_profile_name = prompt_profile_display_name(&prompt_profile);
        let summary = session_summary(
            self.chat_widget.token_usage(),
            self.chat_widget.thread_id(),
            self.chat_widget.thread_name(),
        );
        self.chat_widget.add_plain_history_lines(vec![
            format!("/profile load {}", resolved_path.display())
                .magenta()
                .into(),
        ]);

        if let Some(path) = self.chat_widget.rollout_path().filter(|path| path.exists()) {
            match self
                .server
                .fork_thread(
                    usize::MAX,
                    self.config.clone(),
                    path.clone(),
                    Some(prompt_profile),
                    false,
                    None,
                )
                .await
            {
                Ok(forked) => {
                    self.shutdown_current_thread().await;
                    let init =
                        self.chatwidget_init_for_forked_or_resumed_thread(tui, self.config.clone());
                    self.chat_widget = crate::chatwidget::ChatWidget::new_from_existing(
                        init,
                        forked.thread,
                        forked.session_configured,
                    );
                    self.reset_thread_event_state();
                    if let Some(summary) = summary {
                        let mut lines: Vec<Line<'static>> = vec![summary.usage_line.clone().into()];
                        if let Some(command) = summary.resume_command {
                            let spans =
                                vec!["To continue this session, run ".into(), command.cyan()];
                            lines.push(spans.into());
                        }
                        self.chat_widget.add_plain_history_lines(lines);
                    }
                    self.chat_widget.add_info_message(
                        format!(
                            "Loaded prompt profile `{prompt_profile_name}` from {}.",
                            resolved_path.display()
                        ),
                        Some("It will apply to future turns in this fork.".to_string()),
                    );
                }
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to apply prompt profile from {}: {err}",
                        resolved_path.display()
                    ));
                }
            }
        } else {
            let model = self.chat_widget.current_model().to_string();
            let config = self.fresh_session_config();
            self.shutdown_current_thread().await;
            let report = self
                .server
                .shutdown_all_threads_bounded(Duration::from_secs(10))
                .await;
            if !report.submit_failed.is_empty() || !report.timed_out.is_empty() {
                tracing::warn!(
                    submit_failed = report.submit_failed.len(),
                    timed_out = report.timed_out.len(),
                    "failed to close all threads"
                );
            }
            match self
                .server
                .start_thread_with_tools(config.clone(), Vec::new(), Some(prompt_profile), false)
                .await
            {
                Ok(new_thread) => {
                    let init = crate::chatwidget::ChatWidgetInit {
                        config,
                        frame_requester: tui.frame_requester(),
                        app_event_tx: self.app_event_tx.clone(),
                        initial_user_message: None,
                        enhanced_keys_supported: self.enhanced_keys_supported,
                        auth_manager: self.auth_manager.clone(),
                        models_manager: self.server.get_models_manager(),
                        feedback: self.feedback.clone(),
                        is_first_run: false,
                        feedback_audience: self.feedback_audience,
                        model: Some(model),
                        startup_tooltip_override: None,
                        status_line_invalid_items_warned: self
                            .status_line_invalid_items_warned
                            .clone(),
                        terminal_title_invalid_items_warned: self
                            .terminal_title_invalid_items_warned
                            .clone(),
                        session_telemetry: self.session_telemetry.clone(),
                    };
                    self.chat_widget = crate::chatwidget::ChatWidget::new_from_existing(
                        init,
                        new_thread.thread,
                        new_thread.session_configured,
                    );
                    self.reset_thread_event_state();
                    if let Some(summary) = summary {
                        let mut lines: Vec<Line<'static>> = vec![summary.usage_line.clone().into()];
                        if let Some(command) = summary.resume_command {
                            let spans =
                                vec!["To continue this session, run ".into(), command.cyan()];
                            lines.push(spans.into());
                        }
                        self.chat_widget.add_plain_history_lines(lines);
                    }
                    self.chat_widget.add_info_message(
                        format!(
                            "Loaded prompt profile `{prompt_profile_name}` from {}.",
                            resolved_path.display()
                        ),
                        Some("Greeting seeded for the new session.".to_string()),
                    );
                }
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to start a new session with prompt profile {}: {err}",
                        resolved_path.display()
                    ));
                }
            }
        }

        tui.frame_requester().schedule_frame();
        Ok(AppRunControl::Continue)
    }

    pub(super) async fn handle_show_prompt_profile(&mut self) -> Result<AppRunControl> {
        match self.current_prompt_profile().await {
            Ok(Some(prompt_profile)) => {
                self.chat_widget
                    .add_plain_history_lines(prompt_profile_summary_lines(&prompt_profile));
            }
            Ok(None) => {
                self.chat_widget.add_info_message(
                    "No prompt profile is active for this session.".to_string(),
                    Some(
                        "Use /profile load <path> to import a character card or native prompt profile."
                            .to_string(),
                    ),
                );
            }
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to inspect the current prompt profile: {err}"
                ));
            }
        }
        Ok(AppRunControl::Continue)
    }

    pub(super) async fn handle_clear_prompt_profile(
        &mut self,
        tui: &mut crate::tui::Tui,
    ) -> Result<AppRunControl> {
        match self.current_prompt_profile().await {
            Ok(Some(prompt_profile)) => {
                let prompt_profile_name = prompt_profile_display_name(&prompt_profile);
                let summary = session_summary(
                    self.chat_widget.token_usage(),
                    self.chat_widget.thread_id(),
                    self.chat_widget.thread_name(),
                );
                self.chat_widget
                    .add_plain_history_lines(vec!["/profile clear".magenta().into()]);
                if let Some(path) = self.chat_widget.rollout_path().filter(|path| path.exists()) {
                    match self
                        .server
                        .fork_thread_clearing_prompt_profile(
                            usize::MAX,
                            self.config.clone(),
                            path.clone(),
                            false,
                        )
                        .await
                    {
                        Ok(forked) => {
                            self.shutdown_current_thread().await;
                            let init = self.chatwidget_init_for_forked_or_resumed_thread(
                                tui,
                                self.config.clone(),
                            );
                            self.chat_widget = crate::chatwidget::ChatWidget::new_from_existing(
                                init,
                                forked.thread,
                                forked.session_configured,
                            );
                            self.reset_thread_event_state();
                            if let Some(summary) = summary {
                                let mut lines: Vec<Line<'static>> =
                                    vec![summary.usage_line.clone().into()];
                                if let Some(command) = summary.resume_command {
                                    let spans = vec![
                                        "To continue this session, run ".into(),
                                        command.cyan(),
                                    ];
                                    lines.push(spans.into());
                                }
                                self.chat_widget.add_plain_history_lines(lines);
                            }
                            self.chat_widget.add_info_message(
                                format!("Cleared prompt profile `{prompt_profile_name}`."),
                                Some(
                                    "Future turns in this fork will use the default Codex prompt."
                                        .to_string(),
                                ),
                            );
                        }
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to clear prompt profile `{prompt_profile_name}`: {err}"
                            ));
                        }
                    }
                } else {
                    self.start_fresh_session_with_summary_hint(tui).await;
                    self.chat_widget.add_info_message(
                        format!("Cleared prompt profile `{prompt_profile_name}`."),
                        Some("Future turns will use the default Codex prompt.".to_string()),
                    );
                }
            }
            Ok(None) => {
                self.chat_widget.add_info_message(
                    "No prompt profile is active for this session.".to_string(),
                    Some("The default Codex prompt is already in use.".to_string()),
                );
            }
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to inspect the current prompt profile: {err}"
                ));
            }
        }

        tui.frame_requester().schedule_frame();
        Ok(AppRunControl::Continue)
    }
}
