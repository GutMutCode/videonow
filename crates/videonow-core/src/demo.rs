//! The demo project: a badge (image engine) inside an animated title composition (motion engine)
//! on a sequence over a colour matte (timeline). The CLI's `videonow demo` and the desktop
//! application's demo project both build it, with the same commands an agent would run.

use serde_json::{Value, json};

use crate::Studio;

/// The assets the demo made.
#[derive(Clone, Debug)]
pub struct Demo {
    pub badge: Value,
    pub title: Value,
}

/// Build the demo into `s` (normally an empty project). `step` runs each command and may print
/// or record it; it returns the command's result.
pub fn build(s: &mut Studio, mut step: impl FnMut(&mut Studio, &str, Value) -> Result<Value, String>) -> Result<Demo, String> {
    // 1. image engine: a badge
    let badge = step(s, "studio:image.new", json!({"name": "Badge", "width": 640, "height": 640, "background": "transparent"}))?["asset"].clone();
    step(s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [20, 20, 600, 600], "fill": "#f2b134"}))?;
    step(s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [120, 120, 400, 400], "fill": "#1d2b53"}))?;
    // 2. motion engine: an animated title with the badge in it
    let title = step(s, "studio:comp.new", json!({"name": "Title", "width": 1920, "height": 1080, "fps": 30, "duration": 4}))?["asset"].clone();
    let text = step(s, "motion:layer.newText", json!({"asset": title, "text": "VIDEONOW", "size": 200, "fill": "#ffffff", "position": [960, 900]}))?["layer"].clone();
    step(s, "motion:prop.addKey", json!({"asset": title, "layer": text, "path": "transform/opacity", "time": 0.0, "value": 0}))?;
    step(s, "motion:prop.addKey", json!({"asset": title, "layer": text, "path": "transform/opacity", "time": 1.0, "value": 100}))?;
    let placed = step(s, "studio:image.toComp", json!({"image": badge, "comp": title, "time": 0}))?;
    let badge_layer = placed["layer"]["layer"].clone();
    step(s, "motion:prop.set", json!({"asset": title, "layer": badge_layer, "path": "transform/position", "value": [960, 430]}))?;
    step(s, "motion:prop.addKey", json!({"asset": title, "layer": badge_layer, "path": "transform/scale", "time": 0.0, "value": [0, 0]}))?;
    step(s, "motion:prop.addKey", json!({"asset": title, "layer": badge_layer, "path": "transform/scale", "time": 1.2, "value": [100, 100]}))?;
    // 3. timeline: a sequence with a matte, the title composition on top
    step(s, "timeline:file.newSequence", json!({"name": "Main", "width": 1920, "height": 1080, "fps": 30, "video": 2, "audio": 1}))?;
    let matte = step(s, "timeline:file.newColorMatte", json!({"color": "#20304a", "seconds": 4}))?;
    let matte_item = matte.get("item").cloned().unwrap_or(matte.clone());
    step(s, "timeline:timeline.place", json!({"item": matte_item, "track": "V1", "seconds": 0}))?;
    step(s, "studio:asset.place", json!({"asset": title, "track": "V2", "seconds": 0}))?;
    Ok(Demo { badge, title })
}

/// [`build`] with a step that only runs the command.
pub fn build_quietly(s: &mut Studio) -> Result<Demo, String> {
    build(s, |s, id, p| s.execute(id, p).map(|r| r["result"].clone()).map_err(|e| format!("{id}: {e}")))
}
