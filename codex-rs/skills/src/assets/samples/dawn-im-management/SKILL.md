---
name: dawn-im-management
description: Direct IPC management for capability-gated Dawn IM channels and messaging without MCP tool calls. Use when you need to inspect runtime IM availability, create/delete/register channels, send messages, run IM actions, check status, or run new_chat/fork_chat via the Dawn runtime under ~/.dawn.
---

# Dawn IM Management

This is a Dawn built-in system skill. Codex discovers it from `$CODEX_HOME/skills/.system/dawn-im-management`.

Use `scripts/dawn_im.py` as the single entrypoint.

## Quick Start

```bash
python3 scripts/validate.py
python3 scripts/dawn_im.py im-status --json
python3 scripts/dawn_im.py im-list-channels --im-type discord --json
python3 scripts/dawn_im.py im-send --im-type feishu --channel feishu-my-group --avatar-id Kafka --text "hello" --json
python3 scripts/dawn_im.py context-current --json
python3 scripts/dawn_im.py background-dispatch --items-json '[{"groupFolder":"feishu-ops","avatarId":"Kafka","prompt":"Summarize todays issues","title":"Daily Ops Summary"}]' --json
python3 scripts/dawn_im.py background-status --job-id bg_job_123 --json
```

## Commands

Use `python3 scripts/validate.py` first when you are unsure which IM connectors are present in the active Dawn runtime. Commands without an explicit `--im-type` only fan out across connectors discovered from the active runtime manifest and existing `~/.dawn/ipc/*` roots.

```bash
python3 scripts/dawn_im.py im-create-channel --im-type discord --name "Ops Room" --avatar-id Zome --json
python3 scripts/dawn_im.py im-create-channel --im-type feishu --name "Feishu Ops" --avatar-id Zome --source-group feishu-dawn-test-group --json
python3 scripts/dawn_im.py im-delete-channel --im-type discord --folder "Ops Room" --leave-service --json
python3 scripts/dawn_im.py im-register-channel --im-type feishu --channel-id oc_xxx --avatar-id Kafka --display-name "Team" --json
python3 scripts/dawn_im.py im-react --im-type discord --channel discord-ops --message-id 123 --emoji THUMBS_UP --json
python3 scripts/dawn_im.py im-action --im-type discord --action thread_create --channel discord-ops --params-json '{"name":"subthread"}' --json
python3 scripts/dawn_im.py codex-new-chat --current --json
python3 scripts/dawn_im.py codex-new-chat --im-type feishu --group feishu-ops --avatar-id Kafka --json
python3 scripts/dawn_im.py codex-fork-chat --current --name "Feishu Fork" --json
python3 scripts/dawn_im.py codex-fork-chat --im-type feishu --name "Feishu Fork" --source-group feishu-ops --avatar-id Kafka --json
python3 scripts/dawn_im.py codex-config-read --avatar-id Kafka --json
python3 scripts/dawn_im.py codex-config-read --current --scope effective --json
python3 scripts/dawn_im.py codex-config-set --avatar-id Kafka --model claude-opus-4-1 --approval-policy on-request --json
python3 scripts/dawn_im.py codex-config-set --current --scope group --sandbox-policy danger-full-access --working-directory /tmp/ops --json
python3 scripts/dawn_im.py codex-config-set --group feishu-ops --avatar-id Kafka --scope group --unset sandboxPolicy --json
python3 scripts/dawn_im.py background-dispatch --items-json '[{"groupFolder":"feishu-ops","avatarId":"Kafka","prompt":"Check latest alarms","title":"Alarm Sweep"}]' --json
python3 scripts/dawn_im.py background-status --job-id bg_job_123 --json
python3 scripts/dawn_im.py mcp-call --tool dawn_im_send --args-json '{"im_type":"discord","channel":"discord-ops","text":"hello"}' --json
python3 scripts/dawn_im.py service-restart --json
python3 scripts/dawn_im.py service-restart --service dawn-feishu --json
```

## Feishu Create-Channel Rules

- Feishu `im-create-channel` creates a real Feishu group or private chat. Do not describe it as "register only" or "unsupported".
- Prefer `im-create-channel` over manually assembling a raw `im-action create_channel` call.
- Do not tell the user "Feishu cannot create a group when there is only one member". The correct statement is: with one resolved human member, Feishu creates a private chat (`p2p`) instead of a multi-member group.
- Inside an active Dawn IM conversation, default to the current bound avatar/bot unless the user explicitly names a different avatar.
- If `--source-group` is omitted for Feishu, prefer the current group as the copy source before falling back to other avatar-bound groups.
- In an active Feishu conversation, do not stop and ask for `source_group` before trying the current group. Use current context first.
- If the current Feishu context only resolves one human member, create a private-chat fallback under the current avatar/app. Do not silently switch to another avatar and do not block on a `source_group` question first.
- Ask for explicit `--members` or `--source-group` only when the user clearly wants a real multi-member group and the current chat context is insufficient for that.
- If the user says "I can't see the group", verify with `channel_info` and `member_info`.
- A zero-member result is a failed setup, even if the platform returned a `chatId`.

Decision rule:

- Active Feishu chat + no explicit members/source group + current chat resolves 2+ human members: create a real group from the current chat context.
- Active Feishu chat + no explicit members/source group + current chat resolves 1 human member: create a private chat under the current avatar/app.
- Explicit `--members`: use them.
- Explicit `--source-group`: use it.
- Never stop at "cannot create"; either create the private fallback or explain that a real multi-member group needs additional members.

For active-chat Feishu creation, prefer this shape:

```bash
python3 scripts/dawn_im.py im-create-channel --current --im-type feishu --name "New Room" --json
```

Only ask for explicit `--members` or `--source-group` when the user needs a real multi-member group and the current chat context is not sufficient.

## Direct Send vs Avatar Work

- `im-send` is a raw outbound transport command. Use it for plain text notices, connectivity checks, or debugging.
- For anything that should make an avatar work and reply in-channel, prefer `background-dispatch`.
- For scheduled or system-triggered tasks where the group should see progress and the final result, use `background-dispatch` with `presentationMode="conversation"`.
- On Feishu, pass `--avatar-id` to `im-send` when you know the target avatar. This improves app routing in multi-bot setups.

## Current Chat Operations

For "current chat" actions inside an active Dawn IM conversation, prefer the current-context resolver instead of guessing via `im-list-channels`.

```bash
python3 scripts/dawn_im.py context-current --json
python3 scripts/dawn_im.py codex-new-chat --current --json
python3 scripts/dawn_im.py codex-fork-chat --current --name "Follow-up Thread" --json
```

Rules:

- `context-current` resolves the active IM context from `CODEX_THREAD_ID` and `~/.dawn/ipc/context-bindings/<threadId>.json`.
- `codex-new-chat --current` creates a new thread for the current bound group.
- `codex-fork-chat --current --name ...` forks the current bound thread/group.
- `--group` / `--source-group` are still supported for explicit cross-channel operations.
- Do not use `im-list-channels` to infer the current group unless the task is explicitly cross-channel.

## Codex Config Scopes

Use `codex-config-read` and `codex-config-set` to inspect or update avatar defaults and per-group codex overrides.

```bash
python3 scripts/dawn_im.py codex-config-read --avatar-id Kafka --scope default --json
python3 scripts/dawn_im.py codex-config-read --current --scope group --json
python3 scripts/dawn_im.py codex-config-read --current --scope effective --json
python3 scripts/dawn_im.py codex-config-set --avatar-id Kafka --scope default --model claude-opus-4-1 --model-provider anthropic --json
python3 scripts/dawn_im.py codex-config-set --group feishu-ops --avatar-id Kafka --scope group --sandbox-policy danger-full-access --working-directory /tmp/ops --json
python3 scripts/dawn_im.py codex-config-set --current --scope group --unset sandboxPolicy --json
python3 scripts/dawn_im.py codex-config-set --current --scope group --reset-group-override --json
```

Rules:

- Without `--group` or `--current`, codex config commands target the avatar default config.
- `--current` resolves `avatarId` and `groupFolder` from the active Dawn IM context.
- `--group` without `--avatar-id` only works when that group is bound to exactly one avatar.
- Read scopes:
  - `default`: avatar-level `codex`
  - `group`: explicit `groupConfigs[group].codex`
  - `effective`: avatar default merged with the group override
- Write scopes:
  - `default`: update avatar-level `codex`
  - `group`: update only `groupConfigs[group].codex`
- Group overrides only support `model`, `modelProvider`, `approvalPolicy`, `sandboxPolicy`, and `workingDirectory`.

## Background Dispatch

Use background dispatch when you need to fan out prompts to one or more IM targets and let the avatar reply in-channel without synthesizing fake user messages.

```bash
python3 scripts/dawn_im.py background-dispatch \
  --items-json '[
    {"groupFolder":"feishu-ops","avatarId":"Kafka","prompt":"Summarize the last 3 incidents","title":"Incident Summary"},
    {"groupFolder":"discord-war-room","avatarId":"Zome","prompt":"Post tonight'\''s rollout checklist","title":"Rollout Checklist"},
    {"groupFolder":"feishu-ops","avatarId":"Kafka","prompt":"Review the latest overnight alerts and summarize the risks","title":"Overnight Alert Sweep","presentationMode":"conversation","visibleText":"定时任务：请检查昨夜告警并汇总风险"}
  ]' \
  --json

python3 scripts/dawn_im.py background-status --job-id <jobId> --json
```

Rules:

- `background-dispatch` writes a durable request into `~/.dawn/ipc/im-dispatch/requests`.
- Each item should include `groupFolder`, `avatarId`, and `prompt`. `title`, `metadata`, `presentationMode`, and `visibleText` are optional.
- `presentationMode` defaults to `background_card`:
  - `background_card`: create/update a compact task-status card in-channel
  - `conversation`: post a visible scheduler trigger message in the group, then let the avatar reply using the normal live conversation UX
- In `conversation` mode, `visibleText` controls the visible scheduler trigger text. If omitted, the `prompt` is used.
- The command returns a stable `jobId`. Use `background-status` to inspect item states (`queued`, `running`, `done`, `retry_scheduled`, `failed_terminal`, `delivery_failed`, `canceled`).
- `background-status` also exposes per-item `progress` (`stage`, `label`, `lastActivityAt`, `previewText`) so schedulers can tell whether the avatar is queued, planning, waiting for approval, running tools, or generating the final answer.
- Prefer `background-dispatch` over trying to impersonate a human message when the task is system-triggered or batched.
- Prefer `background-dispatch` over `im-send` when the task should be handled by an avatar rather than delivered as raw text.

## `im-action` Supported Actions

`im-action` is a generic passthrough:

```bash
python3 scripts/dawn_im.py im-action \
  --im-type <whatsapp|feishu|discord> \
  --channel <group-folder> \
  --action <action_name> \
  --params-json '<json_object>' \
  --json
```

`--im-type` values are capability-gated by the active Dawn runtime. Public Dawn builds may omit WhatsApp entirely; treat that as a blocked connector, not as a broken runtime.

`service-restart` uses the Dawn control API. It restarts supervised IM services cross-platform on macOS and Windows without shelling out to `pgrep`, `lsof`, or Unix signals.

### Messaging

- `send_message`: `{ "text": "required", "replyToMessageId": "optional" }`
- `reply`: `{ "messageId": "required", "text": "required" }`
- `edit`: `{ "messageId": "required", "text": "required" }`
- `unsend`: `{ "messageId": "required" }`

### Reactions

- `react`: `{ "messageId": "required", "emoji": "required" }`
- `reactions`: `{ "messageId": "required" }`

### Channels

- `list_channels`: `{}`
- `create_channel`: `{ "name": "required", "avatarId": "required", "members": ["optional"] }`
- `delete_channel`: `{ "leaveService": true|false }`
- `register_channel`: `{ "channelId": "required", "avatarId": "required", "displayName": "optional" }`
- `channel_info`: `{ "channelId": "optional" }`
- `channel_edit`: `{ "name": "optional", "topic": "optional" }`

### Members

- `member_info`: `{ "userId": "optional" }`
- `add_participant`: `{ "userIds": ["required"] }`
- `remove_participant`: `{ "userIds": ["required"] }`
- `leave_group`: `{}`

### Bots

#### Feishu

- `bot_add`: `{ "appName": "required" }` — Add a Feishu app's bot to the group
- `bot_remove`: `{ "appName": "required" }` — Remove a Feishu app's bot from the group
- `bot_list`: `{}` — List which app bots are in the group (requires `--channel`)
- `app_list`: `{}` — List all configured Feishu app names and their default avatars (channel value ignored)

`bot_add` and `bot_remove` target the bot identified by `appName` from the active Feishu connector config under `~/.dawn/connectors/feishu/config.json` (or the resolved runtime config derived from it).

#### Feishu App Configuration

- `app_add`: `{ "name": "required", "appId": "required", "appSecret": "optional", "secretRef": "optional", "defaultAvatarId": "optional" }` — Add a new Feishu app to `~/.dawn/connectors/feishu/config.json` (`appSecret` or `secretRef` required)
- `app_remove`: `{ "name": "required" }` — Remove a Feishu app from `~/.dawn/connectors/feishu/config.json`
- `app_list`: `{}` — List all configured Feishu apps (channel value ignored)

After adding/removing apps, run `service-restart --service dawn-feishu` to apply changes. Public Dawn builds may already manage the secret material through Dawn Settings; when available, prefer existing `secretRef` values over introducing new plaintext secrets.

#### Discord

- `bot_list`: `{ "avatarId": "optional" }` — List all connected bots and their guilds (channel value ignored)
- `bot_invite_url`: `{ "avatarId": "required", "permissions": "optional" }` — Generate OAuth2 invite URL for a bot
- `bot_remove`: `{ "avatarId": "required", "guildId": "required" }` — Bot leaves a guild; also unregisters all channels for that avatar in the guild

**Note:** Discord bots cannot join a guild programmatically. Use `bot_invite_url` to generate a link, then have a guild admin click it to authorize the bot.

### Threads (Discord only)

- `thread_create`: `{ "name": "required", "messageId": "optional" }`
- `thread_list`: `{}`
- `thread_reply`: `{ "threadId": "required", "text": "required" }`

### Moderation (Discord only)

- `kick`: `{ "userId": "required", "reason": "optional" }`
- `ban`: `{ "userId": "required", "reason": "optional", "deleteMessageSeconds": "optional" }`
- `timeout`: `{ "userId": "required", "durationMs": "required", "reason": "optional" }`

### Roles

- `role_info`: `{}`
- `role_add`: `{ "userId": "required", "roleId": "required" }`
- `role_remove`: `{ "userId": "required", "roleId": "required" }`

### Other

- `get_status`: `{}`
- `search`: `{ "query": "required", "limit": "optional" }`
- `set_presence`: `{ "status": "required: online|idle|dnd|invisible" }`
- `emoji_list`: `{}`
- `voice_status`: `{}`

### Action Name Aliases Accepted by `dawn_im.py`

These aliases are normalized before dispatch:

- `channel_info` -> `channel-info`
- `channel_edit` -> `channel-edit`
- `member_info` -> `member-info`
- `add_participant` -> `addParticipant`
- `remove_participant` -> `removeParticipant`
- `leave_group` -> `leaveGroup`
- `bot_add` -> `bot-add`
- `bot_remove` -> `bot-remove`
- `bot_invite_url` -> `bot-invite-url`
- `bot_list` -> `bot-list`
- `app_add` -> `app-add`
- `app_remove` -> `app-remove`
- `app_list` -> `app-list`
- `thread_create` -> `thread-create`
- `thread_list` -> `thread-list`
- `thread_reply` -> `thread-reply`
- `role_info` -> `role-info`
- `role_add` -> `role-add`
- `role_remove` -> `role-remove`
- `set_presence` -> `set-presence`
- `emoji_list` -> `emoji-list`
- `voice_status` -> `voice-status`

Use `im-action --action get_status` to confirm platform-supported actions at runtime.

## `service-restart`

Restart supervised IM services through Dawn Settings' local control API. This path is cross-platform and follows the active runtime capability model on both macOS and Windows.

```bash
python3 scripts/dawn_im.py service-restart \
  --service <dawnclaw|dawn-feishu|dawn-discord|all> \
  --timeout-ms 30000 \
  --json
```

| Flag | Default | Description |
|------|---------|-------------|
| `--service` | `all` | Which service(s) to restart; `all` expands to services detected in the active runtime |
| `--timeout-ms` | `30000` | Timeout passed to the Dawn control API restart request |

`service-restart` does not relaunch Dawn Settings itself. If the desktop shell is down, restart it separately and rerun the command.

## Behavior

- Writes commands to `~/.dawn/ipc/{im}/commands` and polls `command-results`.
- For `codex-new-chat` and `codex-fork-chat`, writes to `~/.dawn/ipc/{im}/requests` and polls `responses`.
- For `background-dispatch`, writes to `~/.dawn/ipc/im-dispatch/requests` and reads durable job files from `~/.dawn/ipc/im-dispatch/jobs`.
- Uses the active Dawn runtime under `~/.dawn/runtime/active.json` and `~/.dawn/runtime/control-api.json` for connector discovery instead of repo-local fallbacks.
- Enforces strict execution verification:
  - All commands must return a command/result response.
  - Channel lifecycle and chat fork/reset commands perform state readback checks.

## Notes

- The script does not require passing `context_token` on the command line.
- Current-chat commands resolve context from `CODEX_THREAD_ID` plus `~/.dawn/ipc/context-bindings/<threadId>.json`, then verify the bound context token before mutating state.
- Reliability comes from post-action verification (`list_channels`, avatar config/state readback, response checks).
- Run `python3 scripts/validate.py` for a non-mutating readiness report with `ready` / `blocked` / `failed`.
- Feishu `im-create-channel` requires either `--members` (open_id list) or `--source-group` so member list can be populated.
- If Feishu create is called without those flags, the script auto-selects an existing Feishu group bound to the avatar as `sourceGroup`; if none exists, it fails fast with a clear error.
- Inside an active Dawn IM conversation, create-channel should prefer the current bound avatar/bot before any implicit default avatar.
- If the current Feishu context only resolves one human member, the allowed fallback is a private chat under the current avatar/app, not a silent switch to another avatar.
- Zero-member Feishu create results are failures and should be treated as failed setup.
- Feishu `codex-fork-chat` now forwards `source-group` into channel creation so the forked group has a valid member source.
- `im-delete-channel --leave-service` is strict: if platform-side deletion/leaving fails, the command fails and local binding is not removed.
- Public Dawn builds may omit WhatsApp (`dawnclaw`) by design. Treat missing WhatsApp as capability-gated behavior, not as a skill failure.
