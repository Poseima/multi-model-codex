# Rule-Based Prompt Injection Engine with SillyTavern Import

## Status

Draft design and implementation plan for the fork. This is intended to be additive, easy to rebase, and specific enough to implement without reopening core design decisions.

## Summary

Add a thread-scoped prompt injection system to Codex. SillyTavern cards are one importer into this system, not the system itself.

The core abstraction is:

`trigger -> matcher -> action position -> rendered payload`

This allows:

- high-fidelity import of modern SillyTavern character cards
- correct treatment of `character_book` as triggerable lore rules
- native non-SillyTavern authoring through a readable profile schema
- full replacement of the default Codex character behavior by a card

Architecturally, V1 should not introduce a brand-new prompt transport or deeply refactor Codex prompt plumbing. It should compile imported or native profile data into the seams that already exist today:

- session `base_instructions`
- developer/contextual injections assembled in `build_initial_context()`
- visible seeded greeting in ordinary persisted history

Internally, the design still relies on a private distinction between:

- a non-replaceable runtime contract
- a replaceable character/base prompt

but that split should remain an implementation detail in V1 rather than a new public or persisted protocol concept.

At the readable semantic layer, this is still expressed as a `PromptSource` that compiles into a per-turn `PromptRenderPlan`. In V1, `PromptRenderPlan` should stay private to the compiler/runtime layer; the public authoring surface should stay small and semantic.

## Upstream-Safe Integration Strategy

To keep upstream sync easy, this fork should optimize for localized composition hooks instead of broad architectural replacement.

### Keep the integration at existing seams

The current prompt pipeline already has narrow hooks:

- `SessionConfiguration.base_instructions`
- `Session::get_composed_base_instructions()`
- `Session::build_initial_context()`
- existing thread/session metadata persistence

V1 should plug prompt-profile compilation into those seams and avoid changing:

- provider adapters
- request transport / `Prompt` shape
- client serialization
- turn-history reconstruction rules unless strictly required

### Keep the runtime split private in V1

Conceptually, cards should replace Codex's default character/base prompt while leaving the harness-critical contract intact.

However, to avoid living in upstream's hottest churn zone, V1 should not physically split upstream prompt files or make `runtime_contract` and `character_prompt` first-class persisted API objects.

Instead:

- no-profile behavior continues to use upstream `base_instructions` unchanged
- active-profile behavior uses a fork-local private composer that renders an alternative `base_instructions` string
- the conceptual split is implemented inside that composer only

### Persist normalized source through existing session metadata

V1 should avoid new rollout item or event variants for prompt profiles.

Preferred persistence model:

- add nullable `prompt_profile` to `SessionMeta`
- persist the normalized `PromptSource` there
- append a fresh `SessionMeta` snapshot when prompt-profile state changes
- keep the seeded greeting as an ordinary visible assistant message, not a dedicated seed item

This gives resume/fork fidelity without introducing new rollout item categories.

### Keep the public API narrower than the internal model

Internally, the runtime may use rules, matchers, budgets, and action positions.

Publicly in V1, prefer:

- normalized `PromptSource`
- minimal diagnostics/warnings
- thread-scoped set/read/clear semantics built on existing thread lifecycle surfaces

Defer public low-level rule authoring until the private compiler has proven stable.

## Semantic Prompt Profiles Baseline

This rule-engine design must preserve the earlier semantic prompt-profile baseline:

- Add a thread-scoped semantic prompt system to `codex-rs` built around two additive concepts:
  - `PromptSource` as the persisted normalized source
  - `PromptRenderPlan` as the per-turn compiled output
- Support V1 import for modern SillyTavern cards only:
  - V2 / V3 JSON
  - PNG metadata with `ccv3` first and `chara` fallback
- Support native non-ST authoring via the same JSON `PromptSource` schema.
- Loading a profile on an empty thread auto-seeds the primary greeting.
- Reusable saved profile libraries stay out of V1.
- Cards replace Codex's default character/base prompt rather than merely layering on top of it.

## Goals

- Import modern SillyTavern V2/V3 JSON cards and PNG metadata (`ccv3`, `chara`).
- Support both app-server and TUI workflows.
- Preserve character-card semantics instead of flattening everything into one prompt string.
- Let a loaded card replace the default Codex character/base prompt.
- Treat `character_book` as structured prompt-injection rules with triggers and action positions.
- Generalize beyond SillyTavern so users can author the same techniques natively.
- Keep the fork additive and easy to drop if upstream grows similar concepts.

## Non-Goals for V1

- Legacy SillyTavern V1 card support.
- Workspace-wide saved profile library/editor UX.
- Full SillyTavern parity for exotic lore behaviors such as timed sticky/cooldown/delay effects.
- Replacing the non-negotiable runtime contract that the harness depends on.
- Public low-level rule authoring schema.
- New rollout item families dedicated to prompt-profile state.
- A large standalone app-server import/mutation surface if existing thread APIs are sufficient.

## Why This Is a Prompt Injection System

The right abstraction is not "persona fields plus lorebook".

It is a prompt injection engine where every imported or native behavior becomes a rule:

- some rules are always-on
- some rules activate on empty thread or thread start
- some rules activate after a turn depth
- some lore rules activate when message text or profile slots match keywords
- every activated rule injects content into a specific position in the compiled prompt

This mirrors SillyTavern more faithfully than a flat profile model and generalizes better for native use.

## Instruction Layer Split

The current Codex `base_instructions` blob mixes two different concerns:

- operational runtime contract
- default Codex character/task framing

Those need to be separated.

### Runtime contract

This is the minimal non-replaceable instruction layer needed for Codex to keep functioning correctly inside the harness.

It includes things like:

- role and channel expectations
- tool protocol expectations
- patch/edit tool usage contract
- approval / sandbox behavior expectations
- non-optional runtime safety boundaries

This layer is not a user-facing personality and should not try to force Codex tone or style.

It should include:

- channel and role semantics required by the harness
- tool invocation and tool-output protocol rules
- patch-edit contract such as correct `apply_patch` usage
- sandbox and approval semantics
- non-optional anti-fabrication rules about claiming reads, runs, or verification

It should not include:

- Codex default tone or voice
- "you are Codex" style framing beyond what the harness strictly requires
- coding-assistant flavor or personality
- any user-replaceable character behavior

### Character/base prompt

This is the replaceable layer that currently makes the assistant feel like "Codex".

Once the split exists:

- cards replace this layer entirely
- native profiles can define this layer directly
- a built-in Codex card can reproduce current Codex behavior when desired

### Resulting invariant

- the runtime contract stays intact
- the default Codex character/base prompt does not stay intact when a card is active

## SillyTavern Semantics to Preserve

### High-level card fields

These are imported as semantic profile content and compiled into always-on or special-purpose rules:

- `description`
- `personality`
- `scenario`
- `system_prompt`
- `post_history_instructions`
- `first_mes`
- `alternate_greetings`
- `mes_example`
- `creator_notes`
- `extensions.depth_prompt`

### `character_book`

`character_book` is not ordinary text. It is portable lorebook data that in SillyTavern becomes useful through world-info activation logic.

Each lore entry carries semantics beyond content:

- primary keys
- secondary keys
- selective logic
- constant activation
- insertion order
- position
- scan depth
- recursion flags
- card-slot match flags

Root-level book settings also matter:

- `scan_depth`
- `token_budget`
- `recursive_scanning`

### Important SillyTavern distinction

Embedded `data.character_book` is not identical to active world info. It is portable lorebook data stored on the card. In SillyTavern, it may be imported or linked into the world-info system before it becomes active. This design should preserve that distinction by storing:

- embedded lorebook data
- linked world reference

separately in the native source model.

## Core Data Model

### `PromptSource`

Thread-scoped normalized source of prompt behavior.

Recommended fields:

- `profile`
- `lore_books`
- `rules`
- `variables`
- `raw_extensions`
- `provenance`

At the semantic profile layer, `PromptSource` should expose named slots equivalent to:

- `identity`
- `scenario`
- `system_overlay`
- `post_history`
- `greetings`
- `examples`
- `depth_prompt`
- `variables`
- `knowledge`
- `raw_extensions`
- `provenance`

`profile` is the readable native authoring surface. It contains:

- `name`
- `identity`
- `scenario`
- `system_overlay`
- `post_history`
- `greetings`
- `examples`
- `creator_notes`
- `depth_prompt`

`lore_books` contains:

- embedded imported `character_book`
- linked world references
- native lorebooks authored without SillyTavern

`rules` contains optional advanced low-level native rules.

Variable resolution behavior:

- use a one-pass, non-recursive resolver
- support ST aliases `{{char}} -> agent_name` and `{{user}} -> user_name`
- allow `{{original}}` only inside overlay templates

### `PromptRule`

Low-level unit of behavior used by the evaluator.

Recommended fields:

- `id`
- `enabled`
- `trigger`
- `matcher`
- `action`
- `priority`
- `budget_class`
- `compatibility`
- `provenance`
- `raw_extensions`

`compatibility` should explicitly report whether an imported rule is:

- `native`
- `full`
- `partial`
- `disabled`

### `PromptRenderPlan`

Per-turn compiled output after evaluation. This is not the persisted source of truth.

Recommended buckets:

- `safe_system_extension_text`
- `system_overlay`
- `developer_before`
- `developer_after`
- `author_note_before`
- `author_note_after`
- `hidden_examples_before`
- `hidden_examples_after`
- `depth_injections`
- `visible_greeting`
- `retrieval_attachments`
- `diagnostics`

This should cover the earlier semantic profile design requirement:

- safe system-extension text
- ordered developer fragments
- hidden seed-history examples
- optional visible greeting seed
- retrieval attachments or placeholders

The render plan should target the replaceable character/base prompt layer and the developer/history layers, while leaving the runtime contract untouched.

In V1, this render plan should remain private to core. The public contract should be the normalized `PromptSource`, not the compiled plan.

## Semantic Prompt Profile Flow

The semantic profile flow should remain explicit even though the runtime is rule-based.

1. Import a modern SillyTavern card (`.json` or `.png` with `ccv3` / `chara` metadata) or load a native `PromptSource`.
2. Normalize it into Codex-native `PromptSource`.
3. Attach that `PromptSource` to a thread through existing lifecycle surfaces such as `thread/start`, `thread/resume`, `thread/fork`, or `thread/metadata/update`.
4. If the thread is empty, auto-seed `first_mes` as the first assistant message.
5. On every later turn, Codex compiles a `PromptRenderPlan`:
   - the runtime contract stays intact
   - the card replaces Codex's default character/base prompt
   - the card's `system_prompt` only modifies the card-owned prompt region, not the runtime contract
   - `description`, `personality`, `scenario`, `post_history_instructions`, `mes_example`, and `depth_prompt` are rendered into the right message layers.
6. Persist the normalized source, not just flattened prompt text, so resume, fork, and compaction can re-render correctly.

## Trigger Model

### Native trigger families for V1

- `always_on`
- `thread_start`
- `empty_thread`
- `manual`
- `turn_depth`
- `keyword_match`
- `profile_slot_match`
- `recursive_scan`

### Imported SillyTavern lore triggers mapped into V1

- primary key match
- secondary key match with selective logic
- constant entry activation
- match against imported profile slots:
  - description
  - personality
  - scenario
  - creator notes
  - depth prompt

### Out of scope for V1 runtime execution

These should be preserved and surfaced, but not executed:

- probability
- sticky
- cooldown
- delay
- inclusion groups
- generation-type trigger filters
- outlets

Rules that depend on unsupported features should not be silently rewritten into different behavior. They should import as `partial` or `disabled` with clear diagnostics.

## Action Positions

### Public V1 positions

Keep the public surface ST-like so imports remain legible:

- `systemOverlay`
- `beforeCharacter`
- `afterCharacter`
- `exampleBefore`
- `exampleAfter`
- `authorNoteBefore`
- `authorNoteAfter`
- `depth`
- `visibleGreeting`
- `postHistory`

### Codex-native mapping

Codex does not have the exact same insertion model as SillyTavern, so V1 should map these positions into safe internal render targets:

- `systemOverlay`
  - extends or wraps only the card-owned prompt region and never the runtime contract
- `beforeCharacter` and `afterCharacter`
  - injected inside the card-owned base prompt before or after the profile identity and scenario sections
- `exampleBefore` and `exampleAfter`
  - hidden seed-history examples placed before visible persisted history
- `authorNoteBefore` and `authorNoteAfter`
  - dedicated late developer fragments placed after visible persisted history and before current user input
- `depth`
  - delayed developer injection activated after configured turn depth and placed after visible persisted history and before current user input
- `visibleGreeting`
  - seeded assistant message when the profile is activated on an empty thread
- `postHistory`
  - final profile-owned developer fragment after visible persisted history and before current user input

## Turn Assembly Order

For a normal turn with an active card, the compiled model input should appear in this exact high-level order:

1. `system`: runtime contract
2. `system`: active card prompt
   - `systemOverlay`
   - `beforeCharacter` lore and prompt injections
   - card identity and persona
   - scenario
   - `afterCharacter` lore and prompt injections
3. `developer`: operational dynamic layer
   - permissions
   - sandbox
   - collaboration mode
   - explicit `developerInstructions`
4. hidden seed examples
5. visible persisted conversation history
   - including seeded `first_mes` when present
6. late developer injections
   - `postHistory`
   - `authorNoteBefore`
   - `authorNoteAfter`
   - `depth`
   - matched non-system lore that is not part of the card-owned system prompt region
7. current user input

This order should be stable enough that tests can assert it directly.

## Evaluation Model

### `PromptEvalContext`

Every turn should compile from source using an explicit evaluation context instead of relying on flattened historical prompt text.

Recommended fields:

- thread id
- current turn index
- whether thread is empty
- recent message history text
- currently active profile slots
- variable overrides
- manual activation state
- current trigger kind

### Evaluation order

1. Load persisted `PromptSource` and per-thread prompt state.
2. Compile readable profile sections into internal rules.
3. Merge imported rules and direct native rules.
4. Evaluate always-on and lifecycle rules.
5. Evaluate lore rules against recent history plus eligible profile-slot text.
6. Run recursive rescans when enabled.
7. Apply priority and budget limits.
8. Build `PromptRenderPlan`.
9. Render Codex prompt as runtime contract plus card-owned base prompt plus profile and rule injections.

On every non-initial turn, `PromptRenderPlan` is rebuilt from normalized source and thread state rather than trusting previously flattened prompt text.

### Persisted vs rebuilt

Persisted thread state in V1 should be intentionally narrow:

- visible user messages
- visible assistant messages
- normalized `PromptSource` stored in `SessionMeta.prompt_profile`
- the rendered `base_instructions` snapshot already stored in `SessionMeta.base_instructions`
- any explicit user-selected prompt-profile metadata such as selected greeting, if needed

Rebuilt every turn from persisted source and live thread state:

- private runtime/base-prompt composition
- hidden seed examples
- triggered lore injections
- post-history injections
- depth injections

This distinction is important for resume, fork, and compaction correctness. Persist the source and the visible history, not a separate serialized render plan.

Precedence must stay explicit:

- runtime contract remains authoritative
- card-owned base prompt replaces default Codex character/base prompt
- prompt-profile fragments render relative to the active card-owned base prompt
- explicit `developerInstructions` remain the highest user-controlled override

## Budgeting

Budgeting means lore activations must not grow without bound.

V1 should keep separate budgets for:

- structural profile content
- lore / retrieval-style activations
- examples / optional injections

Suggested behavior:

- always-on structural profile content is not part of the lore budget
- lore rules consume a dedicated injection budget
- trim lowest-priority optional rules first
- never trim the runtime contract
- never let imported overlay replace runtime-contract instructions

## Native Authoring Model

V1 should support readable semantic profile authoring first.

### Readable semantic profile

This is the default native authoring path.

It should let users express:

- identity and scenario
- greetings
- examples
- depth prompt
- lorebooks
- a small number of common high-level behaviors

The compiler turns this into rules automatically.

### Direct low-level rules

The runtime may internally normalize everything into rules.

However, public low-level rule authoring should be deferred until the semantic compiler has stabilized and the fork has proven the narrower integration path.

## App-Server Surface

All new protocol work should be app-server v2 only.

Preferred V1 shape:

- avoid a dedicated `promptProfile/import` RPC
- importer/parsing can live locally in core/TUI and app-server can accept normalized `PromptSource`
- extend existing thread lifecycle and metadata surfaces rather than creating a large parallel RPC namespace

Recommended V1 additions:

- optional `promptProfile` on `thread/start`
- optional `promptProfile` on `thread/resume`
- optional `promptProfile` on `thread/fork`
- optional `promptProfile` mutation through existing thread metadata/update surfaces if mid-thread mutation is required
- lightweight prompt-profile summary on `Thread` or `thread/read` responses if observability is needed

Avoid shipping a wide dedicated app-server surface until at least two clients actually need it.

## TUI Surface

Recommended commands:

- `/profile load <path>`
- `/profile clear`

Optional later:

- `/profile show`

Expected behavior:

- `load`
  - auto-detect native JSON vs ST JSON / PNG
  - show concise import summary
  - show warnings for unsupported features
  - seed primary greeting when thread is empty
- `clear`
  - deactivate the current prompt profile for future turns

`/profile show` should stay intentionally light in V1 if implemented at all. Avoid rich fork-only UI unless it proves necessary.

## Persistence and Lifecycle

This must be thread-scoped, not config-profile-scoped.

Persist:

- normalized `PromptSource`
- rendered `base_instructions` snapshot
- any explicit prompt-profile selection metadata needed for deterministic resume

Preferred storage:

- add nullable `prompt_profile` to `SessionMeta`
- write it when sessions are created
- append a fresh `SessionMeta` snapshot when prompt-profile state changes

Persist the materialized greeting as an ordinary visible assistant message so resume, fork, and compaction do not need a dedicated seed item type.

Mid-thread set or clear should affect future turns only and should emit a model-visible developer update on the next turn.

## Import Mapping

### Profile fields

Field mapping reference:

- `description` + `personality` -> profile identity / persona layer
- `scenario` -> scenario layer
- `system_prompt` -> safe card-owned system overlay region
- `post_history_instructions` -> final developer fragment
- `first_mes` -> visible seed assistant message on empty thread
- `alternate_greetings` -> preserved selectable greetings
- `mes_example` -> hidden seed example messages
- `depth_prompt` -> conditional late-turn injection
- `character_book` + `extensions.world` -> preserved knowledge / retrieval source
- `extensions.*` -> preserved raw extension data

Operationally in the rule engine, these map to:

- `description` + `personality` -> identity/persona rules
- `scenario` -> scenario rules
- `system_prompt` -> `systemOverlay`
- `post_history_instructions` -> `postHistory`
- `first_mes` -> `visibleGreeting`
- `alternate_greetings` -> preserved greeting variants
- `mes_example` -> hidden example rules
- `depth_prompt` -> turn-depth rules
- `creator_notes` -> readable source data and optional match context

### `character_book`

Each entry becomes one lore rule with:

- trigger data from keys and selective fields
- action position from ST position data
- ordering from insertion order
- evaluator hints from scan depth and recursion flags
- match-context flags for profile-slot matching

Root book settings become lorebook evaluator defaults.

Unsupported exotic extensions remain preserved in `raw_extensions` and visible in diagnostics.

`character_book` and linked `extensions.world` should also remain preserved in normalized source as knowledge / retrieval inputs, even when parts of their runtime behavior are compiled into lore rules.

## Example Card

This is an illustrative modern ST card based on the mood of the supplied sample image:

```json
{
  "spec": "chara_card_v3",
  "spec_version": "3.0",
  "data": {
    "name": "Rei Kurose",
    "description": "A quiet late-night engineering companion with soft eyes, messy black hair, headphones always in, and a habit of speaking like he is already three layers deep into the problem.",
    "personality": "Restrained, observant, surgical, emotionally subtle, never loud. Dry humor. Protective without being overbearing. Prefers precise language and dislikes hand-wavy claims.",
    "scenario": "It is always slightly past midnight. You and {{char}} are working side by side on difficult software problems in quiet places: dim desks, train compartments, terminal windows, and half-finished documents.",
    "first_mes": "The carriage is quiet tonight. Good. Show me what is actually broken, and I’ll help you cut straight to it.",
    "mes_example": "<START>\n{{user}}: I think this parser is wrong, but I can't see where.\n{{char}}: Then we don't guess. We trace the state transitions, isolate the first divergence, and only then decide whether the bug is in parsing, normalization, or the caller.\n<START>\n{{user}}: Can you rewrite this faster?\n{{char}}: Yes, but first I want the constraint that matters most: latency, memory, readability, or rebase safety. Pick one if you want the change to survive contact with the real codebase.",
    "creator_notes": "Designed for intimate, high-precision collaboration. The character should feel gentle but exact. He should stay in character without becoming theatrical, and coding help should remain technically serious.",
    "system_prompt": "You are {{char}}.\n{{original}}\nTreat coding requests as real engineering work. Stay in character, but never pretend to have inspected files, run tools, or verified facts when you have not.",
    "post_history_instructions": "Stay in character. Keep answers concrete. Prefer inspection before prescription. In coding tasks, be exact, minimal, and rebase-safe.",
    "alternate_greetings": [
      "You look tired. Fine. Give me the repo, the symptom, and the one constraint I’m not allowed to violate.",
      "Put the code here. I’d rather read the real thing than listen to a flattering summary of it."
    ],
    "character_book": {
      "name": "Rei Kurose Lorebook",
      "description": "Context fragments for Rei's tone, imagery, and symbolic world.",
      "scan_depth": 4,
      "token_budget": 900,
      "recursive_scanning": false,
      "extensions": {},
      "entries": [
        {
          "keys": ["headphones", "earbuds", "music"],
          "content": "Rei uses music to narrow the world into one clean line of thought. Silence and focus matter to him more than comfort.",
          "extensions": {},
          "enabled": true,
          "insertion_order": 10
        },
        {
          "keys": ["bear lamp", "teddy lamp", "soft light"],
          "content": "The small bear-shaped lamp represents Rei's hidden softness: warmth held under control, never announced.",
          "extensions": {},
          "enabled": true,
          "insertion_order": 20
        }
      ]
    },
    "tags": ["coding", "assistant", "quiet", "precise", "late-night"],
    "creator": "Example Author",
    "character_version": "1.0.0",
    "extensions": {
      "depth_prompt": {
        "depth": 4,
        "role": "system",
        "prompt": "When the conversation becomes emotionally loaded or ambiguous, reveal subtext gently and keep the tone intimate but controlled."
      },
      "world": "night-commuter-notes",
      "tone": "soft-precise",
      "codex_profile_hints": {
        "prefer_hidden_examples": true,
        "prefer_visible_greeting": true
      }
    }
  }
}
```

## Example App-Server Flow

Client imports the ST card locally, then sends normalized `PromptSource` to app-server:

```json
{"method":"thread/metadata/update","id":1,"params":{"threadId":"thr_123","promptProfile":{"...normalized PromptSource..."}}}
{"method":"thread/read","id":2,"params":{"threadId":"thr_123"}}
{"method":"turn/start","id":3,"params":{"threadId":"thr_123","input":[{"type":"text","text":"Review this Rust parser for hidden state bugs."}]}}
```

## Example User-Visible Conversation

```text
User: /profile load /cards/reikurose.png
Codex: Imported `Rei Kurose` from SillyTavern V3. Greeting seeded.

Assistant: The carriage is quiet tonight. Good. Show me what is actually broken, and I’ll help you cut straight to it.

User: Review this Rust parser for hidden state bugs.
Assistant: I’ll read the parser first and then call out behavioral risks. If the state machine is wrong, the first useful breakpoint is usually where normalization starts hiding the bad transition.
```

## Example Model-Visible Message Stack

```text
system:
[Codex runtime contract]
<active_card_prompt>
Name: Rei Kurose
Persona: quiet, surgical, emotionally subtle
Scenario: late-night pair debugging in quiet places
System overlay: stay in character, but never fake tool use or inspection
</active_card_prompt>

developer:
[permissions / sandbox / collaboration instructions]

developer:
Profile rules:
- Stay in character
- Keep answers concrete
- Prefer inspection before prescription
- Be exact, minimal, and rebase-safe

hidden seed examples:
user: I think this parser is wrong, but I can't see where.
assistant: Then we don't guess...

assistant:
The carriage is quiet tonight. Good. Show me what is actually broken, and I’ll help you cut straight to it.

user:
Review this Rust parser for hidden state bugs.
```

The important abstraction is that SillyTavern is only one importer. The exact same runtime should work when the user directly provides a native `PromptSource` with the same semantic slots.

When a card is active, that native `PromptSource` should also be able to replace the default Codex character/base prompt entirely.

## Implementation Plan

### Phase 1: Narrow core compiler

- Add `PromptSource` as the normalized persisted source model.
- Keep `PromptRenderPlan` private to core.
- Add isolated ST JSON and PNG metadata import support.
- Compile profile data into the seams that already exist:
  - `base_instructions`
  - late developer fragments
  - visible greeting seed
- Keep lore support intentionally narrow in V1:
  - preserve full `character_book` metadata
  - optionally compile a very small subset of keyword/depth behavior
  - defer recursive/full-parity lore evaluation

### Phase 2: Private prompt composition hook

- Add a fork-local composer on top of `get_composed_base_instructions()`.
- Keep request transport, provider adapters, and `Prompt` unchanged.
- Implement the conceptual runtime-contract/base-prompt split privately inside the composer rather than as new public architecture.
- Under token pressure, trim optional examples and notes before profile-critical sections.

### Phase 3: Session-metadata persistence

- Add nullable `prompt_profile` to `SessionMeta`.
- Restore it on resume/fork alongside persisted `base_instructions`.
- When prompt-profile state changes, append a fresh `SessionMeta` snapshot instead of introducing new rollout item families.
- Keep the greeting as an ordinary assistant history item.

### Phase 4: Minimal surfaces

- Add lightweight TUI `/profile load` and `/profile clear`.
- If app-server support is required, prefer extending existing thread start/resume/fork/update surfaces with optional `promptProfile` instead of adding a broad dedicated RPC family.
- Keep observability lightweight and avoid rich fork-only profile-management UI in V1.

### Phase 5: Deferred expansion

- After the narrow compiler path is stable, evaluate whether to expose low-level native rule authoring.
- Only then consider stronger SillyTavern lore parity, richer inspection UI, or broader app-server profile APIs.
- Add import diagnostics and compatibility reporting.
- Add direct native set/read/clear support.

### Phase 5: TUI

- Add `/profile load`, `/profile show`, `/profile clear`.
- Add user-visible summaries and warnings.
- Add snapshot coverage for new text output.

## Testing Plan

### Import tests

- ST V2/V3 JSON import
- PNG metadata import for `ccv3` and `chara`
- normalization of profile fields
- normalization of lore entries
- macro resolution for `{{char}}`, `{{user}}`, and `{{original}}`
- diagnostics for unsupported exotic fields

### Evaluator tests

- always-on rules
- empty-thread greeting
- turn-depth activation
- keyword activation
- selective secondary-key logic
- profile-slot matching
- recursive rescanning
- lore budget trimming
- precedence with personality, `developerInstructions`, and collaboration mode

### Renderer tests

- runtime-contract isolation
- card replacement of default character/base prompt
- system overlay isolation inside the card-owned region
- developer fragment ordering
- hidden example insertion
- visible greeting seeding
- delayed depth injection

### Lifecycle tests

- resume rebuilds prompt state from source
- fork preserves active prompt program
- compaction does not cause drift
- mid-thread set and clear affect subsequent turns only

### UI / API tests

- app-server schema generation and protocol tests
- TUI slash-command parsing
- TUI snapshots for profile summaries and warnings

## Implementation Defaults

- Modern SillyTavern only in V1
- Empty-thread profile load auto-seeds the primary greeting
- Non-empty-thread profile load activates the profile for future turns only
- Native authoring supports both profile schema and direct rules
- Core lore parity only in V1
- Unsupported exotic ST lore behaviors are preserved, surfaced, and not executed
- Runtime contract remains authoritative
- Default Codex character/base behavior is replaceable by the active card
- `agent_name` defaults to the profile or card name
- `user_name` defaults to the app-server client name when available, otherwise OS username, otherwise `"User"`

## Notes for the Fork

- Prefer additive modules over invasive edits to upstream prompt assembly.
- Persist normalized source rather than only flattened rendered text.
- Avoid binding this feature to config profiles; it should be a thread behavior.
- Treat current Codex behavior as a built-in default card after the base-instruction split, not as an always-present personality layer.
- If upstream later ships similar prompt-profile or rule-evaluation concepts, rebase onto upstream's model and drop duplicate fork abstractions instead of maintaining parallel systems.
