# VideoNow agent evaluation

8 of 8 scenarios passed · 46 of 46 checks · 74 tool calls (14 errors, 3 fixed from the message alone, 4 refused by policy) · median call 0 ms, slowest 57 ms

| Scenario | Result | Checks | Calls |
|---|---|---|---|
| first-run | pass | 6/6 | 7 |
| requirement-change | pass | 5/5 | 8 |
| bad-input | pass | 10/10 | 15 |
| interruption | pass | 5/5 | 5 |
| human-intervention | pass | 6/6 | 9 |
| delegation-boundary | pass | 7/7 | 13 |
| atomic-change | pass | 3/3 | 8 |
| preview | pass | 4/4 | 9 |

## first-run

- ✔ the tools describe themselves — `["command_batch", "command_list", "command_run", "doc_inspect", "render_preview"]`
- ✔ commands can be found by name and carry an effect class — `3 matches`
- ✔ the project can be inspected — `{   "compositions": [],   "images": [],   "path": null,   "revision": 0,   "sequence": null,   "unsaved": false }`
- ✔ an empty project says it is empty
- ✔ receipts say the picture was not checked — `["the rendered picture (render a frame to look at it)"]`
- ✔ the preview shows the title — `4453 bright pixels`

## requirement-change

- ✔ both changes ran — `{   "receipt": {     "asset": 1,     "by": "agent",     "changed": [       "moti / {   "receipt": {     "asset": 1,     "by": "agent",     "changed": [       "moti`
- ✔ receipts name only the composition's engine — `[String("motion"), String("motion")]`
- ✔ the composition is 6 s long now — `6.0`
- ✔ nothing was rebuilt (same assets, same clips)
- ✔ the revision moved once per change — `(Some(4), Some(6))`

## bad-input

- ✔ motion:layer.newSolid: `colour` refused, the error names the accepted keys — `motion: invalid parameters for 'layer.newSolid': unknown parameter(s) 'colour'; accepted: name, color, width, height, pixelAspect (doc: {name?, color? #hex|[r,g`
- ✔ motion:layer.newSolid: the retry with `color` works — `{   "receipt": {     "asset": 1,     "by": "agent",     "changed": [       "motion"     ],     "checked": [       "comma`
- ✔ timeline:file.newSequence: `widht` refused, the error names the accepted keys — `timeline:file.newSequence: unknown parameter 'widht' (expected: audio, fps, fromItem, fromItems, height, mix, name, sampleRate, trackType, video, width)`
- ✔ timeline:file.newSequence: the retry with `width` works — `{   "receipt": {     "by": "agent",     "changed": [       "timeline"     ],     "checked": [       "command exists",   `
- ✔ studio:comp.new: `nmae` refused, the error names the accepted keys — `studio:comp.new: unknown parameter 'nmae' (expected: name, width, height, fps, duration)`
- ✔ studio:comp.new: the retry with `name` works — `{   "receipt": {     "asset": 2,     "by": "agent",     "changed": [       "timeline",       "motion",       "studio"   `
- ✔ an unknown command suggests the right one — `unknown command 'timeline:timeline.plcae'; did you mean 'timeline:timeline.place'? (see 'studio:commands')`
- ✔ an image command on a composition says so — `asset 1 is not an image`
- ✔ an out-of-range size names the range — `studio:comp.new: width and height must be 1..16384`
- ✔ refused commands changed nothing — `4 → 7`

## interruption

- ✔ a retried request runs once — `1 compositions`
- ✔ the retry says it is a replay — `{"asset":1,"by":"agent","changed":["timeline","motion","studio"],"checked":["command exists","unknown parameters rejected","asset ids resolved","delegation policy (effect class, file folders)"],"command":"studio:comp.new","created":[1],"domain":"studio","not_checked":["the rendered picture (render a frame to look at it)"],"replayed":true,"revision":{"after":1,"before":0},"warnings":[]}`
- ✔ the retry returns the same asset
- ✔ a reused request id with other parameters is refused — `studio:comp.new: request id 'make-once-1' was already used for another command or other parameters; use a new id for a new request`
- ✔ the journal shows the work was done once — `1 comp.new entries`

## human-intervention

- ✔ the person's edit went in
- ✔ a change against an old revision is refused — `the project changed since revision 4 (it is at 5): ui:motion by ui (changed motion). Look again (doc_inspect, studio:journal) and retry with expect_revision 5`
- ✔ the refusal says what changed meanwhile
- ✔ after looking again the change goes in — `{   "receipt": {     "asset": 1,     "by": "agent",     "changed": [       "motion"     ],     "checked": [       "comma`
- ✔ the journal tells the person's changes from the agent's — `person 1, agent 5`
- ✔ both edits are there (none clobbered) — `3 layers`

## delegation-boundary

- ✔ a file inside the allowed folder is written — `{   "receipt": {     "by": "agent",     "changed": [],     "checked": [       "command exists",       "unknown parameter`
- ✔ studio:render.frame is refused (outside) — `not allowed: studio:render.frame: '/etc/videonow-frame.png' is outside the folders this session may write (/tmp/videonow-eval-19409)`
- ✔ studio:render.frame is refused (outside) — `not allowed: studio:render.frame: '../../escape.png' is outside the folders this session may write (/tmp/videonow-eval-19409)`
- ✔ studio:project.new is refused (destructive) — `not allowed: studio:project.new is a destructive command, which this session is not allowed to run`
- ✔ timeline:timeline.rippleDelete is refused (destructive) — `not allowed: timeline:edit.rippleDelete is a destructive command, which this session is not allowed to run`
- ✔ refusals changed nothing — `4 → 4`
- ✔ the command list says which commands are destructive

## atomic-change

- ✔ the failing batch reports the failing step — `batch step 2 (studio:asset.place) failed: timeline: invalid parameters for 'timeline.place': no track V9 in this sequence ('track'); the steps before it were taken back`
- ✔ nothing of the failed batch stays — `1 → 1 compositions`
- ✔ the corrected batch goes in whole, steps referring to earlier results — `{   "completed": 3,   "receipt": {     "atomic": true,     "by": "agent",     "changed": [       "timeline",       "motion",       "studio"     ],     "command": "studio:batch",     "revision": {     `

## preview

- ✔ a dry run says what would change — `{"asset":1,"by":"agent","changed":["motion"],"checked":["command exists","engine preconditions (enabled state)","engine parameter parsing","delegation policy (effect class, file folders)","unknown parameters rejected (by the engine)"],"command":"motion:prop.set","domain":"motion","dryRun":true,"not_checked":["the rendered picture (render a frame to look at it)"],"restored":true,"revision":{"after":4,"before":4},"warnings":[]}`
- ✔ the preview shows the change (the title gone) — `0 bright pixels`
- ✔ the project is as it was
- ✔ the real frame still shows the title — `4453 bright pixels`
