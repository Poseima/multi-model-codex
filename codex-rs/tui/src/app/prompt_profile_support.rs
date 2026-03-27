use std::path::PathBuf;

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
    async fn current_prompt_profile(
        &mut self,
        app_server: &mut crate::app_server_session::AppServerSession,
    ) -> Result<Option<PromptSource>> {
        let Some(thread_id) = self.current_displayed_thread_id() else {
            return Ok(None);
        };
        let thread = app_server
            .thread_read(thread_id, /*include_turns*/ false)
            .await?;
        Ok(thread.prompt_profile)
    }

    pub(super) async fn handle_load_prompt_profile(
        &mut self,
        tui: &mut crate::tui::Tui,
        app_server: &mut crate::app_server_session::AppServerSession,
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

        if let Some(thread_id) = self.current_displayed_thread_id()
            && self
                .chat_widget
                .rollout_path()
                .as_ref()
                .is_some_and(|path| path.exists())
        {
            self.refresh_in_memory_config_from_disk_best_effort("loading a prompt profile")
                .await;
            match app_server
                .fork_thread_with_prompt_profile(
                    self.config.clone(),
                    thread_id,
                    Some(prompt_profile),
                )
                .await
            {
                Ok(forked) => {
                    self.shutdown_current_thread(app_server).await;
                    if let Err(err) = self
                        .replace_chat_widget_with_app_server_thread(tui, app_server, forked)
                        .await
                    {
                        self.chat_widget.add_error_message(format!(
                            "Failed to attach to forked app-server thread: {err}"
                        ));
                    } else {
                        if let Some(summary) = summary {
                            let mut lines: Vec<Line<'static>> =
                                vec![summary.usage_line.clone().into()];
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
                }
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to apply prompt profile from {}: {err}",
                        resolved_path.display()
                    ));
                }
            }
            tui.frame_requester().schedule_frame();
            return Ok(AppRunControl::Continue);
        }

        self.refresh_in_memory_config_from_disk_best_effort("starting a new thread")
            .await;
        let model = self.chat_widget.current_model().to_string();
        let config = self.fresh_session_config();
        let tracked_thread_ids: Vec<_> = self.thread_event_channels.keys().copied().collect();
        self.shutdown_current_thread(app_server).await;
        for thread_id in tracked_thread_ids {
            if let Err(err) = app_server.thread_unsubscribe(thread_id).await {
                tracing::warn!("failed to unsubscribe tracked thread {thread_id}: {err}");
            }
        }
        self.config = config.clone();
        match app_server
            .start_thread_with_prompt_profile(&config, Some(prompt_profile))
            .await
        {
            Ok(started) => {
                if let Err(err) = self
                    .replace_chat_widget_with_app_server_thread(tui, app_server, started)
                    .await
                {
                    self.chat_widget.add_error_message(format!(
                        "Failed to attach to fresh app-server thread: {err}"
                    ));
                } else {
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
                        Some("It will apply to future turns in this session.".to_string()),
                    );
                }
            }
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to start a fresh session with prompt profile {}: {err}",
                    resolved_path.display()
                ));
                self.config.model = Some(model);
            }
        }

        tui.frame_requester().schedule_frame();
        Ok(AppRunControl::Continue)
    }

    pub(super) async fn handle_show_prompt_profile(
        &mut self,
        app_server: &mut crate::app_server_session::AppServerSession,
    ) -> Result<AppRunControl> {
        match self.current_prompt_profile(app_server).await {
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
        app_server: &mut crate::app_server_session::AppServerSession,
    ) -> Result<AppRunControl> {
        match self.current_prompt_profile(app_server).await {
            Ok(Some(prompt_profile)) => {
                let prompt_profile_name = prompt_profile_display_name(&prompt_profile);
                let summary = session_summary(
                    self.chat_widget.token_usage(),
                    self.chat_widget.thread_id(),
                    self.chat_widget.thread_name(),
                );
                self.chat_widget
                    .add_plain_history_lines(vec!["/profile clear".magenta().into()]);

                if let Some(thread_id) = self.current_displayed_thread_id()
                    && self
                        .chat_widget
                        .rollout_path()
                        .as_ref()
                        .is_some_and(|path| path.exists())
                {
                    self.refresh_in_memory_config_from_disk_best_effort(
                        "clearing a prompt profile",
                    )
                    .await;
                    match app_server
                        .fork_thread_clearing_prompt_profile(self.config.clone(), thread_id)
                        .await
                    {
                        Ok(forked) => {
                            self.shutdown_current_thread(app_server).await;
                            if let Err(err) = self
                                .replace_chat_widget_with_app_server_thread(tui, app_server, forked)
                                .await
                            {
                                self.chat_widget.add_error_message(format!(
                                    "Failed to attach to forked app-server thread: {err}"
                                ));
                            } else {
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
                        }
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to clear prompt profile `{prompt_profile_name}`: {err}"
                            ));
                        }
                    }
                } else {
                    self.refresh_in_memory_config_from_disk_best_effort("starting a new thread")
                        .await;
                    let model = self.chat_widget.current_model().to_string();
                    let config = self.fresh_session_config();
                    let tracked_thread_ids: Vec<_> =
                        self.thread_event_channels.keys().copied().collect();
                    self.shutdown_current_thread(app_server).await;
                    for thread_id in tracked_thread_ids {
                        if let Err(err) = app_server.thread_unsubscribe(thread_id).await {
                            tracing::warn!(
                                "failed to unsubscribe tracked thread {thread_id}: {err}"
                            );
                        }
                    }
                    self.config = config.clone();
                    match app_server.start_thread(&config).await {
                        Ok(started) => {
                            if let Err(err) = self
                                .replace_chat_widget_with_app_server_thread(
                                    tui, app_server, started,
                                )
                                .await
                            {
                                self.chat_widget.add_error_message(format!(
                                    "Failed to attach to fresh app-server thread: {err}"
                                ));
                            } else {
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
                                        "Future turns will use the default Codex prompt."
                                            .to_string(),
                                    ),
                                );
                            }
                        }
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to start a fresh session through the app server: {err}"
                            ));
                            self.config.model = Some(model);
                        }
                    }
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
