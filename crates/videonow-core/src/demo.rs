//! The demo project: one small piece that uses all three engines, built only through
//! [`Studio::execute`] the way any client would.
//!
//! A badge (image engine) → an animated title composition with the badge in it (motion engine) →
//! a sequence with a colour matte and the title on top (timeline).

use serde_json::{Value, json};

use crate::{Result, Studio};

/// The assets the demo made.
#[derive(Clone, Debug)]
pub struct Demo {
    pub badge: u64,
    pub title: u64,
}

/// Build the demo into `s` (normally an empty project). `log` sees every step's command and reply.
pub fn build(s: &mut Studio, mut log: impl FnMut(&str, &Value)) -> Result<Demo> {
    let mut step = |s: &mut Studio, id: &str, p: Value| -> Result<Value> {
        let r = s.execute(id, p)?;
        log(id, &r);
        Ok(r["result"].clone())
    };
    let asset = |v: &Value| v["asset"].as_u64().unwrap_or(0);
    // 1. image engine: a badge
    let badge = asset(&step(s, "studio:image.new", json!({"name": "Badge", "width": 640, "height": 640, "background": "transparent"}))?);
    step(s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [20, 20, 600, 600], "fill": "#f2b134"}))?;
    step(s, "image:shape.create", json!({"asset": badge, "kind": "ellipse", "rect": [120, 120, 400, 400], "fill": "#1d2b53"}))?;
    // 2. motion engine: an animated title with the badge in it
    let title = asset(&step(s, "studio:comp.new", json!({"name": "Title", "width": 1920, "height": 1080, "fps": 30, "duration": 4}))?);
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
    let matte_item = matte.get("item").cloned().unwrap_or(matte);
    step(s, "timeline:timeline.place", json!({"item": matte_item, "track": "V1", "seconds": 0}))?;
    step(s, "studio:asset.place", json!({"asset": title, "track": "V2", "seconds": 0}))?;
    Ok(Demo { badge, title })
}
