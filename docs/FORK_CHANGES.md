# Fork Changes

This file records the intentional differences between `Poseima/multi-model-codex`
and `openai/codex`. Update it after every upstream rebase.

## Active Changes

| Area | Main paths | Purpose |
| --- | --- | --- |
| Alternate model providers | `codex-rs/core/src/client.rs`, `codex-rs/core/src/fork_providers.rs`, `codex-rs/model-provider-info/`, `codex-rs/tui/` | Support non-OpenAI providers, Chat Completions compatibility, provider selection, and fork model catalogs. |
| Dawn app-server integration | `codex-rs/app-server/`, `codex-rs/app-server-protocol/` | Expose Dawn collaboration mode, provider metadata, and host-facing protocol extensions. |
| Prompt profiles | `codex-rs/core/`, `codex-rs/tui/` | Load and select reusable prompt profiles. |
| Structured editing | `codex-rs/core/src/tools/handlers/structured_edit.rs`, related tool specifications | Provide the Dawn text-editor and structured-edit workflow. |
| Embedded runtime behavior | `codex-rs/core/`, `codex-rs/config/` | Isolate embedded-mode configuration and preserve host-managed runtime behavior. |
| Memory and compaction behavior | `codex-rs/core/`, `codex-rs/memories/` | Keep Dawn's asynchronous memory and compaction-threshold behavior. |
| Dawn system skill | `codex-rs/core/src/skills/system/dawn/` | Ship Dawn-specific agent instructions in the vendored runtime. |
| Network compatibility | `codex-rs/http-client/`, `codex-rs/codex-api/src/files.rs`, `codex-rs/login/src/auth/` | Preserve environment-proxy and loopback compatibility on top of the upstream HTTP client factory. |
| Local history and telemetry | `codex-rs/core/`, `codex-rs/tui/` | Preserve extended local history, telemetry fields, and Dawn token-usage presentation. |
| Account switching | `codex-rs/tui/src/chatwidget/switch_account.rs` | Switch among auth profiles stored under `CODEX_HOME/multi_auths`. |

## Upstream Sync Notes

### 2026-07-10

- Rebasing onto upstream `1f0566d3f592` replaced direct
  `build_reqwest_client_for_url` calls in realtime, memory, and chat paths with
  upstream `HttpClientFactory` and `build_api_transport` abstractions.
- Dawn's loopback behavior remains enabled for `ReqwestDefault`; system-proxy
  routes remain owned by the upstream factory.
- Raw authentication clients apply the same loopback bypass without enabling
  request or response diagnostics that could expose credentials.
- Upstream header authentication was added to the fork's account-profile
  description logic.
