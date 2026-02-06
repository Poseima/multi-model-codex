# Adding a New Provider and Model

This guide walks through the steps required to add a new model provider and model to the Codex CLI.

## Overview

There are five files to modify:

| File | Purpose |
|------|---------|
| `core/src/model_provider_info.rs` | Provider definition (URL, auth, wire API, etc.) |
| `core/src/models_manager/model_presets.rs` | Model preset shown in the `/model` picker |
| `core/src/models_manager/model_info.rs` | Model metadata (context window, capabilities) |
| `core/src/config/profile.rs` | Built-in profile for quick switching |
| `core/src/config/mod.rs` | *(already wired)* Merges built-in profiles with user config |

---

## Step 1 — Register the provider (`model_provider_info.rs`)

### 1a. Add a provider ID constant

```rust
pub const MY_PROVIDER_ID: &str = "myprovider";
```

Place it in `core/src/fork_providers.rs` next to the existing constants (`OPENROUTER_PROVIDER_ID`, `MINIMAX_PROVIDER_ID`).

### 1b. Create a factory function

```rust
pub fn create_myprovider() -> ModelProviderInfo {
    ModelProviderInfo {
        name: "My Provider".into(),
        base_url: Some("https://api.example.com/v1".into()),
        env_key: Some("MY_PROVIDER_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(4),
        stream_max_retries: Some(10),
        stream_idle_timeout_ms: Some(300_000),
        requires_openai_auth: false,
        system_role: None,                // see note below
    }
}
```

**Key fields:**

- `wire_api` — Use `WireApi::Responses` for the OpenAI Responses API (`/v1/responses`). Chat Completions API was removed upstream.
- `env_key` — The environment variable the user must set with their API key.
- `requires_openai_auth` — Set to `true` only for providers that use OpenAI/ChatGPT login. Almost always `false` for third-party providers.
- `system_role` — If the provider rejects the `"system"` message role, set this to the role it expects (e.g. `Some("user".to_string())`). Leave `None` for standard providers.

### 1c. Register in the built-in provider map

In `register_fork_providers()` in `core/src/fork_providers.rs`, add an entry:

```rust
providers.insert(MY_PROVIDER_ID.into(), create_myprovider());
```

---

## Step 2 — Add a model preset (`model_presets.rs`)

This makes the model appear in the `/model` slash command picker.

### 2a. Register the provider prefix in `fork_provider_mapping.rs`

The provider is derived automatically from the preset `id` prefix. In
`core/src/models_manager/fork_provider_mapping.rs`, add your provider prefix
to the match arm in `provider_for_preset()`:

```rust
match prefix {
    "openrouter" | "minimax" | "volcengine" | "myprovider" => Some(prefix),
    _ => None,
}
```

### 2b. Add a `ModelPreset` entry to the `PRESETS` static

```rust
ModelPreset {
    id: "myprovider/my-model-name".to_string(),
    model: "my-model-name".to_string(),
    display_name: "My Model".to_string(),
    description: "Short description of the model.".to_string(),
    default_reasoning_effort: ReasoningEffort::None,
    supported_reasoning_efforts: vec![],
    supports_personality: false,
    is_default: false,
    upgrade: None,
    show_in_picker: true,
    supported_in_api: true,
},
```

**Key fields:**

- `id` — Unique identifier, conventionally `"provider/model"`. The prefix before the first `/` must match a provider key in `built_in_model_providers()` and `fork_provider_mapping.rs`.
- `model` — The model slug sent to the API in the request body.
- `show_in_picker` — Set to `true` to show in the `/model` UI. Set to `false` for hidden/deprecated models.
- `is_default` — Only one preset across the entire list may be `true`.
- `supported_reasoning_efforts` — Leave empty (`vec![]`) if the model does not support configurable reasoning effort. Otherwise list `ReasoningEffortPreset` entries.

---

## Step 3 — Add model metadata (`model_info.rs`)

This tells the runtime about the model's context window and capabilities. In `find_model_info_for_slug()`, add a branch **before** the final `else` fallback:

```rust
} else if slug.starts_with("my-model") {
    model_info!(
        slug,
        context_window: Some(200_000),           // token limit
        supported_reasoning_levels: Vec::new(),
        default_reasoning_level: None
    )
}
```

The `model_info!` macro creates a `ModelInfo` with sensible defaults. Override only the fields you need. Common overrides:

| Field | When to set |
|-------|-------------|
| `context_window` | Always — determines compaction threshold and footer display |
| `base_instructions` | If the model needs a custom system prompt |
| `supports_reasoning_summaries` | `true` if the model emits reasoning traces |
| `apply_patch_tool_type` | `Some(ApplyPatchToolType::Freeform)` for models that support freeform apply-patch |
| `shell_type` | `ConfigShellToolType::ShellCommand` for models that emit shell commands directly |
| `truncation_policy` | `TruncationPolicyConfig::tokens(N)` or `::bytes(N)` |

---

## Step 4 — Add a built-in profile (`profile.rs`)

Profiles let users switch configurations with `--profile <name>`. In `built_in_profiles()`, add an entry:

```rust
(
    "myprofile".to_string(),
    ConfigProfile {
        model: Some("my-model-name".to_string()),
        model_provider: Some("myprovider".to_string()),
        ..Default::default()
    },
),
```

The profile name is what users pass on the command line: `codex --profile myprofile`.

---

## Step 5 — Verify

```bash
cargo check -p codex-core   # compilation
cargo check                  # full workspace
```

---

## User-side configuration (alternative)

Users can also add providers and profiles without modifying source code, via `~/.codex/config.toml`:

```toml
[model_providers.myprovider]
name = "My Provider"
base_url = "https://api.example.com/v1"
env_key = "MY_PROVIDER_API_KEY"
wire_api = "chat"
system_role = "user"          # optional, if the provider rejects "system" role

[profiles.myprofile]
model = "my-model-name"
model_provider = "myprovider"
```

Built-in providers and profiles take precedence when keys collide. User-defined entries extend but do not override built-ins.

---

## Reference: MiniMax example

The MiniMax provider (`minimax`) and model (`codex-MiniMax-M2.1`) were added following this exact process. Use them as a reference:

- Provider: `create_minimax_provider()` in `fork_providers.rs`
- Preset: `minimax/codex-MiniMax-M2.1` in `model_presets.rs`
- Model info: `slug.starts_with("codex-MiniMax")` branch in `model_info.rs`
- Profile: `m21` in `built_in_profiles()` in `profile.rs`
- System role: set to `"user"` because the MiniMax API does not accept the `"system"` message role
