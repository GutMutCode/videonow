# AGENTS.md — working on VideoNow

VideoNow is one video production application built from three engines copied from FilmCraft,
EffectCraft and PhotoCraft (`engines/`). Read this first.

## Layout

- `crates/videonow-core` — the studio: one session per engine, the asset table, the bridges that
  let one engine render another's document (`bridge.rs`), and `Studio::execute`, the single
  command entry point. Every user-visible action goes through it.
- `crates/videonow-core/src/studio/project_file.rs` — the `.videonow` project folder (save, open,
  verify). `policy.rs` — delegation (effect classes, file roots). `manifest.rs` — every command
  with its effect class. `params.rs` — strict parameter checks. `control.rs` — the JSON requests
  the MCP server and the desktop app's control channel share. `studio/ui.rs` — edits made in an
  engine's own UI, recorded like commands.
- `crates/videonow-automation` — the MCP server, and the control channel (`bridge.rs`). Thin: it
  only passes `control::Request`s to a studio.
- `apps/videonow-cli` — the command line (`demo`, `run`, `exec`, `commands`, `describe`, `mcp`),
  a library plus the `videonow-cli` binary.
- `engines/{film,effect,photo}` — the engines. Their own `AGENTS.md` and `docs/` still describe
  their internals accurately; their rules below still apply inside them.

## Rules

1. **Never crash.** No `unwrap`/`expect`/`panic!` outside tests (the engines' rule, kept). Bad input
   is an error the user or agent can act on.
2. **Everything is a command.** New behaviour is an engine command or a `studio:` command, never
   UI-only logic. Studio commands are declared in `manifest.rs` with their effect class, parameter
   doc and the exact keys they accept (unknown keys are rejected).
3. **Every command returns a receipt.** Do not add a command path that bypasses
   `Studio::execute`; the receipt (what changed, revisions, checked / not checked) is the contract
   with agents and the UI.
4. **Engines stay layered.** Nothing in `engines/*` depends on `videonow-*`. Changes to engine code
   are allowed (we no longer track upstream), but keep them small and marked `Studio:` in a
   comment so they can be found.
5. **Upstream is a reference, not a source of updates.** `engines/UPSTREAM.txt` records the
   commits copied. Do not merge upstream changes wholesale.
6. **No ArtCraft branding.** The logos were removed; do not reintroduce them.
7. **Effect classes are checked.** A new engine command gets its class from its journal flag
   and the words of its id (`manifest::infer_effect`); when that is wrong, add it to
   `manifest::DECLARED`. `tests/manifest.rs` runs every `read` command on the demo project and
   fails if one changes anything.
8. **Parameter docs are contracts.** Unknown keys are refused before a command runs. If an engine
   command really reads a key its doc leaves out, fix the doc; to refresh the generated list of
   such keys run the audit (`scripts/params-audit.py`, see its header) and commit
   `params_extra.rs` and `docs/params-audit.md`.
9. **The project file belongs to the studio.** Engine commands that open, save or close an
   engine's own document are reserved (`manifest::RESERVED`): refused with the studio command to
   use instead.

## Before you finish

```sh
cargo build -p videonow-cli
cargo test -p videonow-core -p videonow-automation
./target/debug/videonow-cli demo /tmp/videonow-demo   # must succeed; look at the PNGs
```
