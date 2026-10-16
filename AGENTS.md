# AGENTS.md — working on VideoNow

VideoNow is one video production application built from three engines copied from FilmCraft,
EffectCraft and PhotoCraft (`engines/`). Read this first.

## Layout

- `crates/videonow-core` — the studio: one session per engine, the asset table, the bridges that
  let one engine render another's document (`bridge.rs`), and `Studio::execute`, the single
  command entry point. Every user-visible action goes through it.
- `crates/videonow-automation` — the MCP server. Thin: it only calls `Studio::execute` on the
  studio's worker thread.
- `crates/videonow-eval` — the agent evaluation: scenarios a reference agent runs over MCP
  (first run, requirement change, bad input, interruption, human intervention, delegation
  boundary, atomic change, preview), with metrics. `cargo test -p videonow-eval` is its gate.
- `apps/videonow-cli` — the `videonow` binary (`demo`, `run`, `commands`, `mcp`, `mcp --attach`).
- `apps/videonow-desktop` — the application window: the three engines' editors as the Edit,
  Motion and Image workspaces, around them the asset list, the command palette and the Agent
  panel. The studio owns the engine sessions and lends each to its editor for a frame
  (`lend.rs`); `Studio::sync_editors` then accounts for what the editor changed.
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
   with agents and the UI. Edits made in an engine's own editor are the one exception, and they
   get their receipt from `Studio::sync_editors` right after the frame (`ui:<workspace>`).
4. **Engines stay layered.** Nothing in `engines/*` depends on `videonow-*`. Changes to engine code
   are allowed (we no longer track upstream), but keep them small and marked `Studio:` in a
   comment so they can be found.
5. **Upstream is a reference, not a source of updates.** `engines/UPSTREAM.txt` records the
   commits copied. Do not merge upstream changes wholesale.
6. **No ArtCraft branding.** The logos were removed; do not reintroduce them.

## Before you finish

```sh
cargo build -p videonow-cli -p videonow-desktop
cargo test -p videonow-core -p videonow-eval
./target/debug/videonow demo /tmp/videonow-demo   # must succeed; look at the PNGs
# the window, offscreen (needs a Vulkan or GL driver; Mesa's lavapipe works):
cargo run -p videonow-desktop --example snapshot -- --demo --out /tmp/videonow.png
cargo test -p videonow-desktop                    # the UI gate: the demo built by clicks alone
```

Golden frames: `crates/videonow-core/tests/golden/`. After a deliberate change to the picture,
look at the new frames, then `VIDEONOW_BLESS=1 cargo test -p videonow-core --test golden`.
