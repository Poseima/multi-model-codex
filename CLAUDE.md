# Repo Objective and Implementation Guideline
- This is a fork of the Codex repository that syncs with upstream daily. The objective is to add powerful new features while making minimal changes to the current codebase. When modifications are absolutely necessary, they are implemented cleanly to facilitate easy rebasing, as large changes can create tedious conflicts during synchronization.
- Prefer additive, well-isolated changes; avoid modifying upstream files unless necessary.
- Keep fork commits atomic and easy to drop if upstream converges.
- If upstream implements a similar feature, drop the fork version or rebase onto upstream’s foundation rather than maintaining duplicates.
- Simple & Effective > Complex & Perfect
- Consistency is the king

# Rust/codex-rs

In the codex-rs folder where the rust code lives:

- Crate names are prefixed with `codex-`. For example, the `core` folder's crate is named `codex-core`
- When using format! and you can inline variables into {}, always do that.
- Install any commands the repo relies on (for example `just`, `rg`, or `cargo-insta`) if they aren't already available before running instructions here.
- Never add or modify any code related to `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR`.
  - You operate in a sandbox where `CODEX_SANDBOX_NETWORK_DISABLED=1` will be set whenever you use the `shell` tool. Any existing code that uses `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` was authored with this fact in mind. It is often used to early exit out of tests that the author knew you would not be able to run given your sandbox limitations.
  - Similarly, when you spawn a process using Seatbelt (`/usr/bin/sandbox-exec`), `CODEX_SANDBOX=seatbelt` will be set on the child process. Integration tests that want to run Seatbelt themselves cannot be run under Seatbelt, so checks for `CODEX_SANDBOX=seatbelt` are also often used to early exit out of tests, as appropriate.
- Always collapse if statements per https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
- Always inline format! args when possible per https://rust-lang.github.io/rust-clippy/master/index.html#uninlined_format_args
- Use method references over closures when possible per https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure_for_method_calls
- When possible, make `match` statements exhaustive and avoid wildcard arms.
- When writing tests, prefer comparing the equality of entire objects over fields one by one.
- When making a change that adds or changes an API, ensure that the documentation in the `docs/` folder is up to date if applicable.
- If you change `ConfigToml` or nested config types, run `just write-config-schema` to update `codex-rs/core/config.schema.json`.

Run `just fmt` (in `codex-rs` directory) automatically after you have finished making Rust code changes; do not ask for approval to run it. Additionally, run the tests:

1. Run the test for the specific project that was changed. For example, if changes were made in `codex-rs/tui`, run `cargo test -p codex-tui`.
2. Once those pass, if any changes were made in common, core, or protocol, run the complete test suite with `cargo test --all-features`. project-specific or individual tests can be run without asking the user, but do ask the user before running the complete test suite.

Before finalizing a large change to `codex-rs`, run `just fix -p <project>` (in `codex-rs` directory) to fix any linter issues in the code. Prefer scoping with `-p` to avoid slow workspace‑wide Clippy builds; only run `just fix` without `-p` if you changed shared crates.

## TUI style conventions

See `codex-rs/tui/styles.md`.

## TUI code conventions

- Use concise styling helpers from ratatui’s Stylize trait.
  - Basic spans: use "text".into()
  - Styled spans: use "text".red(), "text".green(), "text".magenta(), "text".dim(), etc.
  - Prefer these over constructing styles with `Span::styled` and `Style` directly.
  - Example: patch summary file lines
    - Desired: vec!["  └ ".into(), "M".red(), " ".dim(), "tui/src/app.rs".dim()]

### TUI Styling (ratatui)

- Prefer Stylize helpers: use "text".dim(), .bold(), .cyan(), .italic(), .underlined() instead of manual Style where possible.
- Prefer simple conversions: use "text".into() for spans and vec![…].into() for lines; when inference is ambiguous (e.g., Paragraph::new/Cell::from), use Line::from(spans) or Span::from(text).
- Computed styles: if the Style is computed at runtime, using `Span::styled` is OK (`Span::from(text).set_style(style)` is also acceptable).
- Avoid hardcoded white: do not use `.white()`; prefer the default foreground (no color).
- Chaining: combine helpers by chaining for readability (e.g., url.cyan().underlined()).
- Single items: prefer "text".into(); use Line::from(text) or Span::from(text) only when the target type isn’t obvious from context, or when using .into() would require extra type annotations.
- Building lines: use vec![…].into() to construct a Line when the target type is obvious and no extra type annotations are needed; otherwise use Line::from(vec![…]).
- Avoid churn: don’t refactor between equivalent forms (Span::styled ↔ set_style, Line::from ↔ .into()) without a clear readability or functional gain; follow file‑local conventions and do not introduce type annotations solely to satisfy .into().
- Compactness: prefer the form that stays on one line after rustfmt; if only one of Line::from(vec![…]) or vec![…].into() avoids wrapping, choose that. If both wrap, pick the one with fewer wrapped lines.

### Text wrapping

- Always use textwrap::wrap to wrap plain strings.
- If you have a ratatui Line and you want to wrap it, use the helpers in tui/src/wrapping.rs, e.g. word_wrap_lines / word_wrap_line.
- If you need to indent wrapped lines, use the initial_indent / subsequent_indent options from RtOptions if you can, rather than writing custom logic.
- If you have a list of lines and you need to prefix them all with some prefix (optionally different on the first vs subsequent lines), use the `prefix_lines` helper from line_utils.

## Tests

### Snapshot tests

This repo uses snapshot tests (via `insta`), especially in `codex-rs/tui`, to validate rendered output. When UI or text output changes intentionally, update the snapshots as follows:

- Run tests to generate any updated snapshots:
  - `cargo test -p codex-tui`
- Check what’s pending:
  - `cargo insta pending-snapshots -p codex-tui`
- Review changes by reading the generated `*.snap.new` files directly in the repo, or preview a specific file:
  - `cargo insta show -p codex-tui path/to/file.snap.new`
- Only if you intend to accept all new snapshots in this crate, run:
  - `cargo insta accept -p codex-tui`

If you don’t have the tool:

- `cargo install cargo-insta`

### Test assertions

- Tests should use pretty_assertions::assert_eq for clearer diffs. Import this at the top of the test module if it isn't already.
- Prefer deep equals comparisons whenever possible. Perform `assert_eq!()` on entire objects, rather than individual fields.
- Avoid mutating process environment in tests; prefer passing environment-derived flags or dependencies from above.

### Spawning workspace binaries in tests (Cargo vs Bazel)

- Prefer `codex_utils_cargo_bin::cargo_bin("...")` over `assert_cmd::Command::cargo_bin(...)` or `escargot` when tests need to spawn first-party binaries.
  - Under Bazel, binaries and resources may live under runfiles; use `codex_utils_cargo_bin::cargo_bin` to resolve absolute paths that remain stable after `chdir`.
- When locating fixture files or test resources under Bazel, avoid `env!("CARGO_MANIFEST_DIR")`. Prefer `codex_utils_cargo_bin::find_resource!` so paths resolve correctly under both Cargo and Bazel runfiles.

### Integration tests (core)

- Prefer the utilities in `core_test_support::responses` when writing end-to-end Codex tests.

- All `mount_sse*` helpers return a `ResponseMock`; hold onto it so you can assert against outbound `/responses` POST bodies.
- Use `ResponseMock::single_request()` when a test should only issue one POST, or `ResponseMock::requests()` to inspect every captured `ResponsesRequest`.
- `ResponsesRequest` exposes helpers (`body_json`, `input`, `function_call_output`, `custom_tool_call_output`, `call_output`, `header`, `path`, `query_param`) so assertions can target structured payloads instead of manual JSON digging.
- Build SSE payloads with the provided `ev_*` constructors and the `sse(...)`.
- Prefer `wait_for_event` over `wait_for_event_with_timeout`.
- Prefer `mount_sse_once` over `mount_sse_once_match` or `mount_sse_sequence`

- Typical pattern:

  ```rust
  let mock = responses::mount_sse_once(&server, responses::sse(vec![
      responses::ev_response_created("resp-1"),
      responses::ev_function_call(call_id, "shell", &serde_json::to_string(&args)?),
      responses::ev_completed("resp-1"),
  ])).await;

  codex.submit(Op::UserTurn { ... }).await?;

  // Assert request body if needed.
  let request = mock.single_request();
  // assert using request.function_call_output(call_id) or request.json_body() or other helpers.
  ```

## Adding a Non-OpenAI Provider and Model

See `codex-rs/ADDING_PROVIDERS.md` for full step-by-step details with code examples. Below is the quick-reference SOP.

### Summary of current changes (Zhipu example)

The Zhipu (GLM) provider was added across 8 files. Here is what each change does:

| # | File | Change |
|---|------|--------|
| 1 | `core/src/fork_providers.rs` | Provider constant + factory function (`create_zhipu_provider`) + register in `register_fork_providers()` |
| 2 | `core/src/models_manager/fork_provider_mapping.rs` | Add `"zhipu"` to the match arm in `provider_for_preset()` so the TUI model picker can resolve the provider from the preset ID |
| 3 | `core/src/models_manager/model_presets.rs` | Add `ModelPreset` entries (id: `"zhipu/glm-5"`, model: `"glm-5"`, etc.) to the `PRESETS` static |
| 4 | `core/src/models_manager/fork_model_info.rs` | Add model metadata branch (`slug.starts_with("glm-")`) for context window, base instructions, tool type, and shell type |
| 5 | `core/src/config/profile.rs` | Add built-in profiles (`"glm5"`, `"glm47"`) in `built_in_profiles()` |
| 6 | `codex-api/src/provider.rs` | Add `is_zhipu()` detection method (checks provider name or base URL) |
| 7 | `codex-api/src/endpoint/chat_compat.rs` | Inject provider-specific request params (`thinking`, `tool_stream`) before POST |
| 8 | `codex-api/src/sse/chat_compat.rs` | Handle provider-specific SSE fields (`reasoning_content` for Zhipu/DeepSeek) |

Steps 1-5 are required for every provider. Steps 6-8 are only needed if the provider requires non-standard request/response handling.

### Key decisions when adding a provider

- **`wire_api`**: Use `WireApi::Chat` for Chat Completions API providers (most non-OpenAI). Use `WireApi::Responses` only for OpenAI-compatible Responses API.
- **`system_role`**: Set to `Some("user".to_string())` if the provider rejects the `"system"` message role (e.g. MiniMax). Leave `None` for standard providers.
- **`env_key`**: The environment variable name the user must set (e.g. `"ZHIPU_API_KEY"`).
- **`base_url`**: Must NOT include `/chat/completions` — that path is appended automatically by the Chat Completions client.

### Key decisions when adding a model (fork_model_info.rs)

These three fields in `fork_model_info!` must be set consistently — a mismatch causes silent failures (e.g. the model is told about a tool in the prompt but the tool is never registered):

- **`base_instructions`**: Use `BASE_INSTRUCTIONS` (standard, references `apply_patch`) or `BASE_INSTRUCTIONS_WITH_TEXT_EDITOR` (references `text_editor` with `create`/`str_replace`/`delete` JSON commands). The prompt must match the actual tool the model receives.
- **`apply_patch_tool_type`**: Controls which file-editing tool is registered:
  - `None` — falls back to `Freeform` if the upstream feature flag is on, otherwise no file editing tool
  - `Some(ApplyPatchToolType::Freeform)` — `apply_patch` with free-form diff syntax (upstream OpenAI default)
  - `Some(ApplyPatchToolType::Structured)` — `text_editor` tool. **Required** when using `BASE_INSTRUCTIONS_WITH_TEXT_EDITOR`.
- **`shell_type`**: `ConfigShellToolType::Default` uses `exec_command`; `ConfigShellToolType::ShellCommand` uses `shell_command`. Non-OpenAI Chat Completions providers typically use `ShellCommand`.

**Rule**: If `base_instructions` is `BASE_INSTRUCTIONS_WITH_TEXT_EDITOR`, you **must** also set `apply_patch_tool_type: Some(ApplyPatchToolType::Structured)` and typically `shell_type: ConfigShellToolType::ShellCommand`. Always confirm these three fields with the user when adding a new model.

### User-side config.toml (no source changes required)

Users can add providers and switch models entirely via `~/.codex/config.toml` (or `~/.dawn/.codex/config.toml` for Dawn):

```toml
# 1. Define the provider
[model_providers.zhipu]
name = "Zhipu"
base_url = "https://open.bigmodel.cn/api/coding/paas/v4"
env_key = "ZHIPU_API_KEY"
wire_api = "chat"
# system_role = "user"          # uncomment if provider rejects "system" role

# 2. Define a profile for quick switching
[profiles.glm47]
model = "glm-4.7"
model_provider = "zhipu"

# 3. Or set as global default
# model = "glm-4.7"
# model_provider = "zhipu"
```

Then launch with: `codex --profile glm47`

**Important**: `model_provider` must be set alongside `model` in profiles and global config. If only `model` is set, the provider defaults to `"openai"` and the request goes to `api.openai.com`, regardless of the model slug.

### Rebuild after changes

Always rebuild after modifying provider/model source files:
```bash
cd codex-rs && cargo build -p codex-tui
```
Running a stale binary is a common source of "unknown model" errors — the binary must be recompiled to pick up new provider registrations.
