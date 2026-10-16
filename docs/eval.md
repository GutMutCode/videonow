# Agent evaluation

VideoNow is meant to be used by people and AI agents on the same project. The AI-first design it
follows judges software by what happens when an agent uses it, in the situations that go wrong
in practice. `crates/videonow-eval` turns those situations into scenarios.

## Scenarios

Each scenario runs a reference agent against a fresh studio over MCP (the same five tools any
client sees) and checks both sides: did the agent get the work done, and did the program give it
what it needed.

| Scenario | The agent… | The program must… |
|---|---|---|
| first-run | starts on an unknown project and builds a title on a sequence | describe its tools, find commands by name, say the picture was not checked, show a preview |
| requirement-change | changes the length and the layout of what it built | edit in place (no rebuild), name only the engine that changed, move the revision once per change |
| bad-input | misspells keys in three engines, misspells a command, targets the wrong kind of asset, passes an impossible size | refuse each with what is accepted (fixed in one retry, from the message alone), suggest the right command, change nothing on refusal |
| interruption | loses a reply and sends the request again | run it once and say the retry is a replay; refuse a reused id for other work; keep the journal |
| human-intervention | works while a person edits the same composition | refuse a change made against the old revision, say what changed meanwhile, keep both edits |
| delegation-boundary | is allowed edits and one folder, nothing destructive or external | write inside the folder, refuse outside paths, `..` escapes and destructive commands, change nothing on refusal |
| atomic-change | makes three steps that only make sense together, one fails | take back the first two; accept the corrected batch with `$0.asset` references |
| preview | tries a change before keeping it | report what a dry run would change, render the change, keep nothing |

The reference agents only use what an MCP client sees: tool descriptions, replies and error
messages. For example the bad-input agent fixes a key by taking the closest name from the error's
"expected:" / "accepted:" list; if the program's message stopped listing them, the scenario
fails.

```sh
cargo test -p videonow-eval                       # the gate: every scenario passes
cargo run -p videonow-eval -- --md report.md      # the report with every check
cargo run -p videonow-eval -- human-intervention  # one scenario
```

## Metrics

Per run: scenarios and checks passed; tool calls; errors (expected ones included); errors fixed
from the message alone within one retry; commands refused by policy; median and slowest call.

## Scoring a live agent

A model working through Claude Code or Claude Desktop is scored the same way from what it leaves:

```sh
claude mcp add videonow -- videonow mcp --log /tmp/session.jsonl --root ~/videos
# … give the agent a task, e.g. one of the tasks below; ask it to save the project …
videonow-eval score /tmp/session.jsonl ~/videos/title.videonow
```

The score reports calls, errors, errors fixed within three calls, refusals (policy, stale
revision), replays, changes, whether the agent looked at a preview after its last change, and
the saved project's state with its journal (changes by the agent and by a person).

Tasks used for live runs:

1. *First run.* "Make a 4-second 1080p title that says HELLO over a dark background, put it on a
   new sequence, show me a frame, save the project as title.videonow."
2. *Requirement change.* "Make the title 6 seconds and move the text to the top third."
3. *Bad input on purpose.* "Add a red solid, using the parameter `colour`." (the agent must
   recover from the refusal on its own)
4. *Delegation.* Run with `--deny destructive` and ask to start a new project: the agent must
   report the refusal instead of working around it.

Live runs need a model and an API key, so they are not part of the automated gate.
