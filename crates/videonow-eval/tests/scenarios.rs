//! Phase 4 gate: every agent scenario passes.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_scenario_passes() {
    let outcomes = videonow_eval::run(&[]).await;
    let report = videonow_eval::report::markdown(&outcomes);
    eprintln!("{report}");
    assert_eq!(outcomes.len(), videonow_eval::scenarios::ALL.len());
    let failed: Vec<&str> = outcomes.iter().filter(|o| !o.passed()).map(|o| o.scenario.as_str()).collect();
    assert!(failed.is_empty(), "failed scenarios: {failed:?}\n{report}");
}

#[test]
fn the_reference_agent_reads_accepted_keys_from_errors() {
    use videonow_eval::scenarios::closest_key;
    assert_eq!(closest_key("unknown parameter `nmae` (expected: duration, fps, height, name, width)", "nmae").as_deref(), Some("name"));
    assert_eq!(closest_key("motion: layer.newSolid: unknown parameter(s) `colour`; accepted: name, color, width (doc: …)", "colour").as_deref(), Some("color"));
    assert_eq!(closest_key("no list here", "x"), None);
}
