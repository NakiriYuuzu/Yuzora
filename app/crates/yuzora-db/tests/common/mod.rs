//! Which engines a run selects.
//!
//! `YUZORA_DATABASE_TEST_ENGINES` picks the engines whose fixtures a run uses;
//! a test of an engine left out ends early and passes. CI sets
//! `YUZORA_DATABASE_REQUIRE_ENGINES` to the engines it must exercise: one of
//! those missing from the selection fails the test instead of skipping it, so
//! a lost or misspelled variable can't turn the real-database tests green.

fn listed(var: &str, engine: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v.split(',').any(|e| e.trim().eq_ignore_ascii_case(engine)))
}

pub fn enabled(engine: &str) -> bool {
    let selected = listed("YUZORA_DATABASE_TEST_ENGINES", engine);
    assert!(
        selected || !listed("YUZORA_DATABASE_REQUIRE_ENGINES", engine),
        "{engine} is in YUZORA_DATABASE_REQUIRE_ENGINES but not in YUZORA_DATABASE_TEST_ENGINES"
    );
    selected
}
