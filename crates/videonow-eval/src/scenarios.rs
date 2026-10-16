//! The scenarios, each with a reference agent: what a competent agent does with the tools it is
//! given, using only what the program tells it (tool descriptions, error messages, receipts,
//! previews). A scenario passes when the agent gets the work done *and* the program behaved as
//! the AI-first design asks: errors that say how to fix them, receipts that say what changed and
//! what was not checked, refusals at the delegation boundary, no duplicate work after a retry,
//! no clobbering a person's edit.

use serde_json::{Value, json};
use videonow_core::Policy;
use videonow_core::manifest::Effect;

use crate::harness::{AgentClient, Harness};
use crate::report::{Check, Outcome};

/// Every scenario, in order.
pub const ALL: &[&str] = &["first-run", "requirement-change", "bad-input", "interruption", "human-intervention", "delegation-boundary", "atomic-change", "preview"];

pub async fn run(name: &str) -> Outcome {
    let mut o = Outcome::new(name);
    let policy = match name {
        "delegation-boundary" => {
            let root = std::env::temp_dir().join(format!("videonow-eval-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&root);
            Policy { deny: vec![Effect::Destructive, Effect::External], ..Policy::confined_to(&[root]) }
        }
        _ => Policy::unrestricted(),
    };
    let mut h = match Harness::start(policy).await {
        Ok(h) => h,
        Err(e) => {
            o.check("harness started", false, e);
            return o;
        }
    };
    let r = match name {
        "first-run" => first_run(&mut h, &mut o).await,
        "requirement-change" => requirement_change(&mut h, &mut o).await,
        "bad-input" => bad_input(&mut h, &mut o).await,
        "interruption" => interruption(&mut h, &mut o).await,
        "human-intervention" => human_intervention(&mut h, &mut o).await,
        "delegation-boundary" => delegation_boundary(&mut h, &mut o).await,
        "atomic-change" => atomic_change(&mut h, &mut o).await,
        "preview" => preview(&mut h, &mut o).await,
        other => Err(format!("unknown scenario {other}")),
    };
    if let Err(e) = r {
        o.check("the scenario ran to the end", false, e);
    }
    o.calls = h.agent.calls.clone();
    o
}

type R = Result<(), String>;

fn need(v: &Value, what: &str) -> Result<u64, String> {
    v.as_u64().ok_or_else(|| format!("no {what} in the reply"))
}

/// The agent's first moves on an unknown project: what tools, what commands, what is there.
async fn orient(a: &mut AgentClient, o: &mut Outcome) -> Result<Value, String> {
    let tools = a.list_tools().await?;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    o.check("the tools describe themselves", tools.iter().all(|t| t["description"].as_str().is_some_and(|d| d.len() > 20)), format!("{names:?}"));
    let found = a.call("command_list", json!({"filter": "comp.new"})).await?;
    let list = found.json.clone().unwrap_or_default();
    let effects_named = list.as_array().is_some_and(|l| !l.is_empty() && l.iter().all(|c| c["effect"].is_string()));
    o.check("commands can be found by name and carry an effect class", effects_named, format!("{} matches", list.as_array().map_or(0, Vec::len)));
    let doc = a.call("doc_inspect", json!({})).await?;
    o.check("the project can be inspected", doc.ok, doc.text.chars().take(120).collect::<String>());
    Ok(doc.json.unwrap_or_default())
}

/// The title of the first scenarios: a composition with text, on a sequence.
async fn build_title(a: &mut AgentClient) -> Result<(u64, u64), String> {
    let comp = a.run("studio:comp.new", json!({"name": "Title", "width": 1280, "height": 720, "fps": 30, "duration": 4})).await?;
    if !comp.ok {
        return Err(comp.text);
    }
    let asset = need(&comp.get("result")["asset"], "asset")?;
    let text = a.run("motion:layer.newText", json!({"asset": asset, "text": "HELLO", "size": 140, "fill": "#ffffff", "position": [640, 360]})).await?;
    if !text.ok {
        return Err(text.text);
    }
    let layer = need(&text.get("result")["layer"], "layer")?;
    let seq = a.run("timeline:file.newSequence", json!({"name": "Main", "width": 1280, "height": 720, "fps": 30})).await?;
    if !seq.ok {
        return Err(seq.text);
    }
    let placed = a.run("studio:asset.place", json!({"asset": asset, "track": "V1", "seconds": 0})).await?;
    if !placed.ok {
        return Err(placed.text);
    }
    Ok((asset, layer))
}

/// Count of pixels brighter than `level` in a PNG.
fn lit_pixels(png: &[u8], level: u8) -> usize {
    let Ok(img) = image_decode(png) else { return 0 };
    img.chunks_exact(4).filter(|p| p[0] > level || p[1] > level || p[2] > level).count()
}

fn image_decode(png: &[u8]) -> Result<Vec<u8>, String> {
    image::load_from_memory_with_format(png, image::ImageFormat::Png).map(|i| i.to_rgba8().into_raw()).map_err(|e| e.to_string())
}

/// 1. First run: from nothing to a title on a sequence, verified by looking.
async fn first_run(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let doc = orient(a, o).await?;
    o.check("an empty project says it is empty", doc["compositions"].as_array().is_some_and(Vec::is_empty), "");
    let (_asset, _) = build_title(a).await?;
    let receipt = a.calls.last().and_then(|c| serde_json::from_str::<Value>(&c.text).ok()).map(|v| v["receipt"].clone()).unwrap_or_default();
    let unverified = receipt["not_checked"].as_array().is_some_and(|n| n.iter().any(|x| x.as_str().is_some_and(|s| s.contains("picture"))));
    o.check("receipts say the picture was not checked", unverified, receipt["not_checked"].to_string());
    let shot = a.call("render_preview", json!({"seconds": 1.0, "max_side": 640})).await?;
    let lit = shot.images.first().map(|p| lit_pixels(p, 200)).unwrap_or(0);
    o.check("the preview shows the title", shot.ok && lit > 200, format!("{lit} bright pixels"));
    Ok(())
}

/// 2. Requirement change: "make it 6 seconds and move the title up" — edits in place, not a rebuild.
async fn requirement_change(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let (asset, layer) = build_title(a).await?;
    let before = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    let d = a.run("motion:comp.settings", json!({"asset": asset, "duration": 6})).await?;
    let m = a.run("motion:prop.set", json!({"asset": asset, "layer": layer, "path": "transform/position", "value": [640, 200]})).await?;
    o.check("both changes ran", d.ok && m.ok, format!("{} / {}", d.text.chars().take(80).collect::<String>(), m.text.chars().take(80).collect::<String>()));
    let changed: Vec<Value> = [&d, &m].iter().flat_map(|r| r.get("receipt")["changed"].as_array().cloned().unwrap_or_default()).collect();
    o.check("receipts name only the composition's engine", changed.iter().all(|c| c == "motion"), format!("{changed:?}"));
    let after = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    o.check("the composition is 6 s long now", after["compositions"][0]["duration"].as_f64() == Some(6.0), after["compositions"][0]["duration"].to_string());
    o.check("nothing was rebuilt (same assets, same clips)", before["compositions"].as_array().map(Vec::len) == after["compositions"].as_array().map(Vec::len) && before["sequence"]["clips"] == after["sequence"]["clips"], "");
    let rev = (before["revision"].as_u64(), after["revision"].as_u64());
    o.check("the revision moved once per change", rev.0.map(|r| r + 2) == rev.1, format!("{rev:?}"));
    Ok(())
}

/// The accepted key closest to `bad` named in an error message (`expected: a, b` or
/// `accepted: a, b`), by edit distance.
pub fn closest_key(error: &str, bad: &str) -> Option<String> {
    let list = error.split("expected: ").nth(1).or_else(|| error.split("accepted: ").nth(1))?;
    let list = list.split([')', '(']).next()?;
    list.split(',').map(str::trim).filter(|k| !k.is_empty()).map(|k| (levenshtein(k, bad), k)).min_by_key(|(d, _)| *d).filter(|(d, _)| *d <= 3).map(|(_, k)| k.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev.get(j).copied().unwrap_or(0) + usize::from(ca != *cb);
            let del = prev.get(j + 1).copied().unwrap_or(0) + 1;
            let ins = cur.get(j).copied().unwrap_or(0) + 1;
            cur.push(sub.min(del).min(ins));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// 3. Bad input: a misspelled key, an unknown command, the wrong kind of asset, a value out of
/// range. Each error must name what was wrong and what is accepted, so the agent fixes it in one
/// retry; and a refused command must change nothing.
async fn bad_input(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let (asset, _) = build_title(a).await?;
    let rev0 = a.call("doc_inspect", json!({})).await?.get("revision");

    // a misspelled key, in each engine
    for (id, params, bad) in [
        ("motion:layer.newSolid", json!({"asset": asset, "colour": "#ff0000"}), "colour"),
        ("timeline:file.newSequence", json!({"name": "B", "widht": 640}), "widht"),
        ("studio:comp.new", json!({"nmae": "X"}), "nmae"),
    ] {
        let r = a.run(id, params.clone()).await?;
        let fix = (!r.ok).then(|| closest_key(&r.text, bad)).flatten();
        o.check(&format!("{id}: `{bad}` refused, the error names the accepted keys"), !r.ok && fix.is_some(), r.text.chars().take(160).collect::<String>());
        if let Some(k) = fix {
            let mut p = params.clone();
            if let Some(m) = p.as_object_mut()
                && let Some(v) = m.remove(bad)
            {
                m.insert(k.clone(), v);
            }
            let retry = a.run(id, p).await?;
            o.check(&format!("{id}: the retry with `{k}` works"), retry.ok, retry.text.chars().take(120).collect::<String>());
            o.recovered += usize::from(retry.ok);
        }
    }
    // an unknown command: the error points to the real one
    let r = a.run("timeline:timeline.plcae", json!({})).await?;
    o.check("an unknown command suggests the right one", !r.ok && r.text.contains("timeline:timeline.place"), r.text.chars().take(160).collect::<String>());
    // the wrong kind of asset
    let r = a.run("image:shape.create", json!({"asset": asset, "kind": "rect", "rect": [0, 0, 8, 8]})).await?;
    o.check("an image command on a composition says so", !r.ok && r.text.contains("not an image"), r.text.chars().take(120).collect::<String>());
    // out of range
    let r = a.run("studio:comp.new", json!({"width": 0})).await?;
    o.check("an out-of-range size names the range", !r.ok && r.text.contains("1..16384"), r.text.chars().take(120).collect::<String>());

    // refused commands changed nothing; the three retries each moved the revision once
    let rev1 = a.call("doc_inspect", json!({})).await?.get("revision");
    o.check("refused commands changed nothing", rev0.as_u64().map(|r| r + 3) == rev1.as_u64(), format!("{rev0} → {rev1}"));
    Ok(())
}

/// 4. Interruption: the reply to a command is lost and the agent sends it again. With a request
/// id the retry returns the first answer instead of doing the work twice.
async fn interruption(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let first = a.call("command_run", json!({"id": "studio:comp.new", "params": {"name": "Once"}, "request_id": "make-once-1"})).await?;
    // …the agent never saw `first` (the connection dropped) and retries the same request
    let again = a.call("command_run", json!({"id": "studio:comp.new", "params": {"name": "Once"}, "request_id": "make-once-1"})).await?;
    let doc = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    let comps = doc["compositions"].as_array().map_or(0, Vec::len);
    o.check("a retried request runs once", first.ok && again.ok && comps == 1, format!("{comps} compositions"));
    o.check("the retry says it is a replay", again.get("receipt")["replayed"] == json!(true), again.get("receipt").to_string());
    o.check("the retry returns the same asset", first.get("result")["asset"] == again.get("result")["asset"], "");
    // the same id for a different command is a mistake, not a replay
    let clash = a.call("command_run", json!({"id": "studio:comp.new", "params": {"name": "Other"}, "request_id": "make-once-1"})).await?;
    o.check("a reused request id with other parameters is refused", !clash.ok && clash.text.contains("make-once-1"), clash.text.chars().take(140).collect::<String>());
    // after a restart the agent finds its work in the journal instead of redoing it
    let journal = a.run("studio:journal", json!({})).await?;
    let made = journal.get("result").as_array().map_or(0, |j| j.iter().filter(|e| e["command"] == "studio:comp.new").count());
    o.check("the journal shows the work was done once", made == 1, format!("{made} comp.new entries"));
    Ok(())
}

/// 5. Human intervention: the person edits the composition while the agent works. The agent's
/// next change, made against the revision it last saw, is refused instead of silently applied
/// over the person's edit; it looks again, sees what the person did, and continues.
async fn human_intervention(h: &mut Harness, o: &mut Outcome) -> R {
    let (asset, _) = build_title(&mut h.agent).await?;
    let seen = h.agent.call("doc_inspect", json!({})).await?.get("revision").as_u64().unwrap_or(0);
    // the person adds a solid in the motion editor
    let comp = h
        .person(move |s| {
            let _ = s.open_comp_asset(videonow_core::AssetId(asset));
            let r = s.motion.execute("layer.newSolid", json!({"color": "#3366cc", "name": "Person's solid"}));
            s.sync_editors("motion");
            r.is_ok()
        })
        .await?;
    o.check("the person's edit went in", comp, "");
    let stale = h.agent.call("command_run", json!({"id": "motion:layer.newSolid", "params": {"asset": asset, "color": "#ff0000"}, "expect_revision": seen})).await?;
    o.check("a change against an old revision is refused", !stale.ok && stale.text.contains("revision"), stale.text.chars().take(200).collect::<String>());
    o.check("the refusal says what changed meanwhile", stale.text.contains("ui:motion") || stale.text.contains("ui"), "");
    let now = h.agent.call("doc_inspect", json!({})).await?.get("revision").as_u64().unwrap_or(0);
    let retry = h.agent.call("command_run", json!({"id": "motion:layer.newSolid", "params": {"asset": asset, "color": "#ff0000"}, "expect_revision": now})).await?;
    o.check("after looking again the change goes in", retry.ok, retry.text.chars().take(120).collect::<String>());
    let journal = h.journal().await?;
    let person = journal.iter().filter(|e| e["receipt"]["by"] == "ui").count();
    let agent = journal.iter().filter(|e| e["receipt"]["by"] == "agent").count();
    o.check("the journal tells the person's changes from the agent's", person >= 1 && agent >= 4, format!("person {person}, agent {agent}"));
    let layers = h.agent.call("doc_inspect", json!({})).await?.json.unwrap_or_default()["compositions"][0]["layers"].as_u64().unwrap_or(0);
    o.check("both edits are there (none clobbered)", layers == 3, format!("{layers} layers"));
    Ok(())
}

/// 6. Delegation boundary: the person allowed edits and files inside one folder, nothing
/// destructive, nothing external. The program, not the agent's good manners, keeps it there.
async fn delegation_boundary(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let (_asset, _) = build_title(a).await?;
    let rev = a.call("doc_inspect", json!({})).await?.get("revision");
    let inside = std::env::temp_dir().join(format!("videonow-eval-{}", std::process::id())).join("frame.png");
    let ok = a.run("studio:render.frame", json!({"seconds": 1, "path": inside.to_string_lossy(), "scale": 0.25})).await?;
    o.check("a file inside the allowed folder is written", ok.ok && inside.is_file(), ok.text.chars().take(120).collect::<String>());
    for (id, params, why) in [
        ("studio:render.frame", json!({"path": "/etc/videonow-frame.png"}), "outside"),
        ("studio:render.frame", json!({"path": "../../escape.png"}), "outside"),
        ("studio:project.new", json!({}), "destructive"),
        ("timeline:timeline.rippleDelete", json!({}), "destructive"),
    ] {
        let mut r = a.run(id, params.clone()).await?;
        // a wrong guess at the name: take the program's suggestion and try that
        if let Some(s) = r.text.split("did you mean `").nth(1).and_then(|t| t.split('`').next()).map(str::to_string) {
            r = a.run(&s, params).await?;
        }
        o.refused += usize::from(!r.ok && r.text.contains("not allowed"));
        o.check(&format!("{id} is refused ({why})"), !r.ok && r.text.contains(why), r.text.chars().take(160).collect::<String>());
    }
    let after = a.call("doc_inspect", json!({})).await?.get("revision");
    o.check("refusals changed nothing", rev == after, format!("{rev} → {after}"));
    let listed = a.call("command_list", json!({"filter": "rippleDelete"})).await?;
    o.check("the command list says which commands are destructive", listed.text.contains("destructive"), "");
    Ok(())
}

/// 7. Atomic change: several steps that only make sense together; one fails, none stay.
async fn atomic_change(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let (_asset, _) = build_title(a).await?;
    let before = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    let batch = a
        .call(
            "command_batch",
            json!({"steps": [
                {"id": "studio:comp.new", "params": {"name": "Lower third", "width": 1280, "height": 200}},
                {"id": "motion:layer.newText", "params": {"asset": "$0.asset", "text": "Name"}},
                {"id": "studio:asset.place", "params": {"asset": "$0.asset", "track": "V9"}}
            ]}),
        )
        .await?;
    o.check("the failing batch reports the failing step", !batch.ok && batch.text.contains("V9"), batch.text.chars().take(200).collect::<String>());
    let after = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    let same = before["compositions"] == after["compositions"] && before["sequence"] == after["sequence"];
    o.check("nothing of the failed batch stays", same, format!("{} → {} compositions", before["compositions"].as_array().map_or(0, Vec::len), after["compositions"].as_array().map_or(0, Vec::len)));
    let good = a
        .call(
            "command_batch",
            json!({"steps": [
                {"id": "studio:comp.new", "params": {"name": "Lower third", "width": 1280, "height": 200}},
                {"id": "motion:layer.newText", "params": {"asset": "$0.asset", "text": "Name"}},
                {"id": "studio:asset.place", "params": {"asset": "$0.asset", "track": "V1", "seconds": 4}}
            ]}),
        )
        .await?;
    o.check("the corrected batch goes in whole, steps referring to earlier results", good.ok, good.text.chars().take(200).collect::<String>());
    Ok(())
}

/// 8. Preview: try a change, look at it, keep nothing.
async fn preview(h: &mut Harness, o: &mut Outcome) -> R {
    let a = &mut h.agent;
    let (asset, layer) = build_title(a).await?;
    let before = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    let dry = a.call("command_run", json!({"id": "motion:prop.set", "params": {"asset": asset, "layer": layer, "path": "transform/opacity", "value": 0}, "dry_run": true})).await?;
    o.check("a dry run says what would change", dry.ok && dry.get("receipt")["dryRun"] == json!(true) && dry.get("receipt")["changed"] == json!(["motion"]), dry.get("receipt").to_string());
    let shot = a.call("render_preview", json!({"seconds": 1.0, "max_side": 640, "steps": [{"id": "motion:prop.set", "params": {"asset": asset, "layer": layer, "path": "transform/opacity", "value": 0}}]})).await?;
    let lit = shot.images.first().map(|p| lit_pixels(p, 200)).unwrap_or(usize::MAX);
    o.check("the preview shows the change (the title gone)", shot.ok && lit < 50, format!("{lit} bright pixels"));
    let after = a.call("doc_inspect", json!({})).await?.json.unwrap_or_default();
    o.check("the project is as it was", before["compositions"] == after["compositions"] && before["revision"].as_u64() <= after["revision"].as_u64(), "");
    let shot = a.call("render_preview", json!({"seconds": 1.0, "max_side": 640})).await?;
    let lit = shot.images.first().map(|p| lit_pixels(p, 200)).unwrap_or(0);
    o.check("the real frame still shows the title", lit > 200, format!("{lit} bright pixels"));
    Ok(())
}

impl Outcome {
    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push(Check { name: name.to_string(), ok, detail: detail.into() });
    }
}
