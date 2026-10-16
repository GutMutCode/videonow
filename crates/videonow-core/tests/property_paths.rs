use serde_json::{Value, json};
use videonow_core::{Policy, Studio};

#[test]
fn confined_motion_edits_resolve_property_paths_without_file_access() {
    let mut studio = Studio::new();
    studio.set_policy(Policy::default());
    let composition = studio.execute("studio:comp.new", json!({"width": 16, "height": 16})).unwrap()["result"]["asset"].clone();
    let layer = studio.execute("motion:layer.newSolid", json!({"asset": composition, "color": "#ffffff"})).unwrap()["result"]["layer"].clone();
    assert!(layer.is_number());
    let edited = studio.execute("motion:prop.set", json!({"asset": composition, "layer": layer, "path": "transform/opacity", "value": 40})).unwrap();
    assert_eq!(edited["receipt"]["changed"], json!(["motion"]));
    let property = studio.execute("motion:prop.get", json!({"asset": composition, "layer": layer, "path": "transform/opacity"})).unwrap();
    assert_eq!(property["result"]["value"], json!(40.0));

    studio.execute("motion:prop.addKey", json!({"asset": composition, "layer": layer, "path": "transform/opacity", "time": 0, "value": 40})).unwrap();
    let selected =
        studio.execute("motion:keys.select", json!({"asset": composition, "keys": [{"layer": layer, "path": "transform/opacity", "time": 0}]})).unwrap();
    assert!(selected.get("receipt").is_some());
    let revision = studio.revision();
    let error = studio.execute("motion:essential.templateInfo", json!({"path": "outside.ectemplate"})).unwrap_err().to_string();
    assert!(error.contains("no readable folder"), "{error}");
    assert_eq!(studio.revision(), revision);
    assert_eq!(
        studio.execute("motion:prop.get", json!({"asset": composition, "layer": layer, "path": "transform/opacity"})).unwrap()["result"]["value"],
        Value::from(40.0)
    );
}
