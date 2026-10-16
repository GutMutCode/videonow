# Status

Where VideoNow stands against the roadmap in the design document (phases 1–4), what each
phase's gate checks, and what is not done. Updated with each phase.

## Gates

| Phase | Gate | How it is checked | Result |
|---|---|---|---|
| 1 Engine cleanup | a reopened project renders the same; the engines' tests pass | `tests/studio.rs` (save → open → byte-identical frame), `tests/golden.rs`; engine suites | pass (engine suites: 2 known failures, below) |
| 2 One window | a person builds the demo project through the window alone | `apps/videonow-desktop/tests/ui_gate.rs`: clicks, keys and drags only; every journal entry `by: ui` | pass |
| 3 Deep integration | golden frames identical; playback within budget | `tests/golden.rs` (3 frames, deterministic, survive save/open; half-res frame after an edit under the budget), `tests/requests.rs` | pass |
| 4 Agents | every agent scenario passes | `crates/videonow-eval` (8 scenarios over MCP) | pass |

## Phase 1 — engine cleanup

- `.videonow` project folders: manifest, timeline, compositions, images, and the journal.
  Files are read and checked before anything is replaced; a failed open changes nothing.
- `MediaRef::Nested` in the timeline engine: compositions and images stay live after reopening.
- One time crate (EffectCraft uses FilmCraft's `Tick`).
- Strict parameters for every engine command (calibrated on 10,296 engine test calls), effect
  classes for all commands (read, edit, destructive, fileWrite, external), delegation policy
  (effect classes, read/write folders, `..` and symlinks resolved).
- Engine commands that save or replace one engine's document are refused with the studio
  command to use (`HOST_COMMANDS`).

## Phase 2 — one window

- `videonow-desktop`: the three engines' editors as the Edit, Motion and Image workspaces, the
  asset list, the command palette (with dry run), the Agent panel (agent access over a localhost
  MCP port with a token, permissions, activity log of receipts, undo).
- An agent attaches to the open window (`videonow mcp --attach`) and edits the project the
  person sees; its receipts say `by: agent`, the panel's permissions apply, and a connection
  without the token is closed (`tests/agent_attach.rs`).
- The studio owns the engine sessions and lends each to its editor for a frame; edits made in
  an editor become `ui:<workspace>` receipts with the engine commands they ran.
- Documents made in an editor become assets and are offered to the other engines; image tabs
  closed in the image editor keep their document.
- Editors' Save / Open / New go to the shell, which saves or opens the whole project.
- VideoNow name, icon and mark; the ArtCraft community buttons and links are gone from the
  editors.

## Phase 3 — deep integration

- Compositions and images reach the timeline as premultiplied linear float frames (values above
  1 kept); 8-bit colours survive the round trip exactly.
- Composition audio joins the timeline mix (the motion engine's mixdown at 48 kHz).
- Requests: `request_id` (a retry returns the first answer), `expect_revision` (a change made
  against an old revision is refused with what changed since), atomic `studio:batch` with
  `$N.path` references, dry runs, `render_preview` with `steps` (look at a change, keep nothing).
- Dry runs, previews with steps, and atomic batches preflight the whole request and reject
  file writes, external effects, project replacement, history changes, and opaque command
  wrappers that cannot be restored by a document checkpoint. Save/export separately after
  confirming edits, or use an explicitly non-atomic batch when partial effects are intended.
- Exact document rollback preserves the prior saved/unsaved state despite engine revision
  counters advancing during undo; incomplete rollback never marks the project saved.
- The resolved default render path and previous project save path are checked before writing.
  Command-specific property paths are distinguished from file paths, including nested keyframe,
  pick-whip, and inline preset properties.
- Misspelled commands get a suggestion.
- Golden frames for the demo; a playback budget test.

## Phase 4 — agents and packaging

- `videonow-eval`: 8 scenarios (first run, requirement change, bad input, interruption, human
  intervention, delegation boundary, atomic change, preview) run by reference agents over MCP,
  with metrics; `videonow-eval score` scores a live agent session from `videonow mcp --log`.
- `flake.nix` (package, app, dev shell, check) and a desktop entry.

## Not done, or not verified here

- **Live model runs.** The scenarios are run by reference agents. Scoring a model (Claude Code,
  Claude Desktop) works from its session log, but no live run was made here: it needs a model
  and an API key (`docs/eval.md`).
- **Web build.** The engines have web front ends of their own; the VideoNow shell and the
  studio have not been built for WebAssembly.
- **The flake** parses, but was not built here: the sandbox cannot reach the Nix binary cache
  or GitHub archives.
- **Engine suites**: two FilmCraft tests fail in this environment (APV export, which the
  installed ffprobe 6.1 cannot read; one demo-project subsequence test). The same two failed
  in the other phase-1 branch on unchanged code. Two EffectCraft media tests failed because the
  engines now share FilmCraft's decoders, which already turn rotated video upright and put AAC
  surround in WAV order; EffectCraft did both again. Fixed with two small `Studio:` patches
  (`probe.rs`, `pool.rs`); the EffectCraft media suite passes.
- **Audio playback in the window**: the timeline editor runs without an audio device here
  (no `cpal` output wired); composition audio reaches the mix and exports.
- **Colour, text and raster libraries are still per engine.** Frames cross between engines in
  one well-defined format (premultiplied linear float); merging the libraries themselves was not
  needed for any gate and was left out.
- **Autosave and crash recovery** are the engines' own and are not wired for the project as a
  whole; save often.
- The playback budget is measured on a debug build on a two-core machine (generous 4 s); a
  release build is far faster.
