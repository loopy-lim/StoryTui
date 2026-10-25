# Known Limitations

A frank list of what StoryTUI does **not** do as of W11. Read this alongside
[RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md) (the open runtime
items) and [INSTALL.md](./INSTALL.md).

## Interactive host loop is connected

The binary `apps/storytui` now runs the interactive event pump:
`crossterm` event stream → `App::handle_key` → `EngineCommand` → `reduce` →
`provider.generate_turn` → `AcceptGeneration` → publish view. Provider
failures restore the last stable checkpoint and render a recoverable error.
Automated coverage already proves:

- The full engine flow against a `MockProvider` over a 100-turn synthetic
  soak (`crates/story-core/tests/soak.rs`).
- A 3-turn rainy-reunion Mock E2E that exercises opening + 3 player turns,
  cancel-mid-generation, and a malformed-draft recovery
  (`apps/storytui/tests/mock_e2e.rs`).
- The diagnostic bundle assembly + redacted serialization
  (`apps/storytui/tests/diagnostic_bundle.rs`).
- The deterministic benchmark runner
  (`apps/storytui/tests/bench.rs`).

The host loop is playable with a reachable, authenticated OpenAI-compatible
server. A live first-scene generation has been verified against the checked-in
OMLX profile. A real 100-turn session is still an open release gate because the
automated long-run suite uses MockProvider.

## MVP quality metrics are pending a real model

Every numeric quality target (prose, voice, rule respect, sovereignty) in
the PRD requires a real MLX server. The Mock cannot score on these axes —
its drafts are scripted. The code-level gates that ARE in place:

- 0 sensitive-text leaks in default logs/diagnostics (`tests/redact.rs`).
- Deterministic prompt + patch hashes (`tests/bench.rs`).
- 100-turn synthetic soak + 100-iteration save round-trip.

The runtime trace records only structural provider metadata, never a raw model
response excerpt, and is capped at 1 MiB. Pack file reads resolve canonical
targets and reject paths outside the canonical Pack root. Save slot ids are
portable filename stems (`[A-Za-z0-9_-]`, 1–64 characters), not paths.

Closing the real-model quality gate is documented in
`RELEASE-RUNTIME-GATE.md` (G3).

## Model license and safe-pack principles

StoryTUI is built for local, player-controlled fiction. The project itself
ships no model weights and takes no position on which MLX model an operator
chooses to load. Two principles the code enforces:

1. **Supported ratings only.** A pack must use `general` or `teen`. The
   checked-in packs and prompts target an all-ages presentation, and the
   loader rejects unsupported rating values before play starts.
2. **Player sovereignty.** The system prompt (see
   `crates/story-core/src/prompt_context.rs`) instructs the model not to
   fix emotions, dialogue, physical contact, forgiveness, confessions, or
   decisions the player did not specify. Free-text actions reinforce this
   in Korean. A model that ignores this instruction is a model-quality
   issue the engine cannot fully prevent.

## Streaming is the two-stage path, not the single-pass default

The `OpenAiCompatibleProvider` implements both `direct` (Director) and
`write_stream` (Writer SSE) — see W10. The single-call path
(`generate_turn`) does not stream tokens to the renderer: its response is
one structured JSON draft (narration + choices + patch), so there is no
prose to reveal early — the loading overlay reflects the whole call. The
streaming experience lives in the optional 2-stage Director → Writer path
(`EngineState::use_director_writer`): the Director plans silently, then the
Writer's narration streams into the transcript live. Players toggle that
path in Settings (`g`) or persist it in `user-settings.toml`; the shipped
default stays single-pass (ADR-005) until the real-model A/B gate passes.

## Expression sprites are keyword-classified, not model-judged

Character expression sprites (`pack/images/expressions/<character-id>/<emotion>.png`,
with the character portrait as the fallback) are picked by a zero-inference
keyword classifier over each turn's narration (`story_tui::classify_emotion`)
— deliberately NOT by a separate classifier model the way SillyTavern's
Expressions extension runs one. A weak local setup pays no extra generation
for expressions, and a misread degrades to "no sprite / neutral", never to
wrong story state. The tradeoff: the rule table reads surface cues only
(Korean-led, a few English cues) — sarcasm, negation, and who is speaking are
out of reach, and narration in other languages classifies as neutral. The
Settings `f` toggle turns sprites off without touching scene images (`i`).

## Manual image protocol forcing can garble unsupported terminals

Scene images follow the terminal capability probed once at startup
(`Picker::from_query_stdio`), and the Settings `b` cycle can force a protocol
(`kitty`/`sixel`/`iterm2`/`halfblocks`, persisted in `user-settings.toml`).
Forcing kitty/sixel/iterm2 on a terminal that does not actually support them
prints raw graphics escape bytes — the display needs a redraw or restart to
recover; auto (the default) never does this. Without a detected base picker
only halfblocks is forceable, since the other protocols need the queried cell
size. A protocol switch re-encodes every scene image immediately, but
graphics already painted by the previous protocol may linger until the
terminal clears them (kitty/sixel images live outside the text grid).

## Cross-pack runtime evidence is still limited

Two packs are committed: `stories/rainy-reunion` and
`stories/starlight-station`. Both pass loader validation, while only
`rainy-reunion` has been exercised through the app-level integration path.
The 10-turn live-model swap to `starlight-station` remains runtime gate G5.

Pack loading now fails closed for future `min_engine_version`, choice counts
outside 1–6, duplicate identifiers, invalid numeric/stat ranges, and locations
that cannot be reached from the initial location through declared adjacency.
This is conservative structural validation, not a proof that every conditional
event or ending will be reached in a real playthrough.

## Non-goal: cloud providers

The provider abstraction is `OpenAiCompatible`. Cloud providers that need
proprietary auth flows, SDKs, or non-OpenAI wire shapes are intentionally
out of scope. A cloud provider that speaks the OpenAI shape (e.g.
`openrouter`) will work; one that does not, will not.

## Undo is one-key, with the ring persisted in saves

The story screen's `u` rewinds the most recently committed turn (the AI
Dungeon "rewind" / SillyTavern "swipe" equivalent): the previous turn's
narration and choices come back and the autosave slot is rewritten with the
rewound state, so quitting and resuming never resurrects an undone turn.
Bounds, all deliberate:

- The checkpoint ring keeps **8 committed turns** and now travels inside the
  save payload (`SavePayload.undo_stack`), so `u` works right after loading a
  save. Old saves without the field behave exactly as before — the ring fills
  as turns commit. The ring stores full state snapshots, so saves got larger.
- There is no redo/history tree (NovelAI-style branching): undo always steps
  back one turn, and re-committing after an undo pushes a fresh checkpoint —
  the stale same-turn entry can never be restored into a redo by accident.
  Branching today means "undo, pick differently, then `s` to save the branch";
  the undone turn's narration is kept as a **variant** — `[` / `]` on the
  story screen swipes the re-committed turn through kept alternatives
  (session memory only: restarting drops the variant list, the shown text
  stays). A full tree UI remains future work.
- Undo is refused while a generation is in flight or an error overlay is up;
  the failed-retry slot is cleared so an old turn cannot replay on top of the
  rewound state.

## Re-rolls restart the request; in-flight drafts are not cached

`r` while loading re-issues the same turn from the same checkpoint with a
fresh generation id (the AI Dungeon retry / SillyTavern swipe moment), and
`r` on an error overlay retries the failed one — each `r` is a new model
call and the in-flight draft is never cached. Committed variants, however,
are kept: after undo + re-commit, `[` / `]` swipes the turn's narration
through the kept alternatives (rewriting the autosave), so comparing
alternatives no longer needs the redo/history cycle.

## Automation assists are bounded; manual fallbacks stay one step away

Since 2026-10-23 the app takes a few actions on its own, each with a hard
cap — defense must not stall the flow (a length floor once caused a
near-per-turn regeneration at ~2x latency and was disabled for exactly that
reason, see `config/storytui.toml`):

- **Auto-resume** — with an autosave present, startup lands directly in the
  restored story (notice: "이전 세션을 이어요"; `Home` reopens the start menu).
  `auto_resume = false` in user-settings.toml restores the menu-first
  startup, and a fresh install (no autosave) always sees the menu.
- **Auto-retry once** — a failed generation is re-issued once ~1.5s later
  with a status line before the [R]/[Esc] overlay appears. The second
  failure is always manual, player cancellation (`c`) never auto-retries,
  and each `r` remains a fresh model call.
- **Server recovery probing** — while the provider is unavailable and no
  generation is in flight, the app re-probes every ~15s, so a returning
  server flips the status surfaces (home notice, settings projection) without
  opening Settings. The story screen itself has no server-status surface —
  a failed turn there still shows the error overlay until retried.
- **Truncation continue** — a Writer turn that ends at the token cap
  (finish_reason "length") gets one automatic continue append per turn
  ("✎ 이어씀" badge); if a narration repair already ran that turn the hint
  is manual-only (`w`). There is deliberately no minimum-length floor. Like
  the kept turn variants, the badge and the empty-side flag are session
  memory — restarting the app clears them.
- **Empty side-result retry** — a continue/scene/suggest generation that
  returns empty narration is re-dispatched once, then reported via a toast;
  side generations still never move state or the turn count.

The current Settings contract is a TOML profile plus CLI and per-save-directory
user overrides. The in-app model picker reads `/v1/models` and persists the
main selection to `saves/user-settings.toml`; Director/Choice/Writer
assignments are per provider profile — keyed by profile name in
`[stage_models.<name>]`, restored when the profile is switched to, with
`[[providers]]` entries able to carry per-profile defaults (precedence:
user-settings profile entry, then the profile default, then the file-wide
default). Old flat `director_model`/`choice_model`/`writer_model` user
settings are folded into the active profile on load. If a persisted model
disappears, startup and switches repair it to an advertised model. Response
style is editable in-app as a preset cycle (`x`: 자동 → 창의적 → 균형 → 정밀),
which decides temperature/top_p/top_k while active and persists in
user-settings; 자동 — the default — keeps the profile's own tuning. The player
identity is editable in-app too (`a`: name + persona), overriding the base
`[player]` profile per field and applying to the pack prompt from the next
turn. Two more steering surfaces joined the same contract: an author's note
(`t`) injected verbatim into every turn's system prompt, and the lorebook
dialog (`w`) — view entries, toggle `enabled`, edit content/keys; edits apply
to the very next turn, but persist only when the pack ships its
`lorebook.toml` as a file (otherwise the edit is session-only, and the dialog
says so). Non-steering extras: `w` on the story screen continues the last
narration without a new turn (choices and state stay put; single-pass only —
the two-stage path is not combined), `z` asks the model for a one-line
suggested player action into the free-input buffer, `r` in Settings shows the
last sent system/user prompts (session memory, never persisted), and
`storytui export <slot> [--format txt|jsonl]` writes a save out as a
portable transcript. Group-slice extras on the story screen: `g` asks the
model for one group-conversation scene bit (`이름: "대사"` lines) appended to
the last narration, and `@캐릭터 내용` free input pins the targeted character
(`p` opens the read-only cast roster). Raw sampler values, provider URL, authentication,
timeouts, retry budget, context budget, and save directory still come from
the base profile or CLI; editing those exact numbers in the TUI remains a
future UX improvement (deliberately — one preset key instead of five
sliders). Two TOML-only power knobs follow the same policy (no UI, no keys,
off by default): `[[display_rules]]` — at most 16 regex display rewrites
applied to newly shown transcript text, invalid patterns are skipped with a
warning — and `[quick_actions]`, where `"1" = "텍스트"` binds Alt+1..9 to fill
the input buffer.

## Frontend-parity gaps that remain

The 2026-10-23 survey (docs/FRONTEND-COMPARISON.md) brought SillyTavern-style
lorebook editing, an author's note, continue, impersonate, a prompt viewer,
chat export, display rules, quick actions, variant swiping, and the group
slice into the TUI. Still open:

- **Group chat / multi-character conversation** — the minimal slice landed
  (2026-10-23): `g` attaches one cast-driven scene bit to the last narration
  and `@캐릭터 내용` targets one character, but a scene **never changes
  relationship state** (the draft's state patch is discarded by design — only
  committed player turns move the meters), and per-turn actor selection with
  per-cast commits remains an engine and pack-schema redesign, deliberately
  out of scope.
- The lorebook dialog edits keys (`\,` escapes a literal comma), content,
  `enabled`, `constant` (`c`), and probability (`p` cycles 100/75/50/25);
  sticky/cooldown, recursion flags, and insertion positioning stay
  editor-in-TOML territory. The suspected activation-timer defect (ledger
  lost on accept) was fixed in the 2026-10-23 follow-up cycle — timers now
  survive accepts, retries, and undo/re-commits.
