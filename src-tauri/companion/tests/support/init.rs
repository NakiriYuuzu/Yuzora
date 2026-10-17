pub fn json() -> String {
    let mut init: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/init.json")).unwrap();
    let sentinel = std::env::temp_dir()
        .join("yzc-nonexistent-herdr")
        .join("DO-NOT-EXECUTE-HERDR");
    assert!(sentinel.is_absolute());
    assert!(!sentinel.exists());
    init["herdrBinary"] = serde_json::json!(sentinel);
    init.to_string()
}
