//! The resource chip that every layout shows (src/yuzora/dock.rs `place_perf`) draws a sample: the shipped
//! `yuzora-perf` never reads `layout()`, so which layouts host it, and where, is tested in `yuzora::dock` and in
//! `window::tests::every_shipped_layout_shows_the_resource_chip_in_the_corner`, not here.
use super::*;

/// `yuzora:perf` with one sample in it (1% CPU, 203 MB): the real one samples processes on a thread of its own.
fn one_sample() -> HostModule {
    let usage = json!({"cpu": 1.0, "memory": 203_000_000.0, "count": 1});
    let state = json!({
        "snapshot": {"total": usage, "app": usage, "core": usage, "tools": usage, "core_pid": 42, "core_error": null},
        "health": {"attempts": 1, "failures": 0, "empty": 0, "skipped": 0, "last_error": null},
        "cost": null,
    });
    let read = state.clone();
    HostModule::new(yuzora_perf::NAME)
        .declarations(yuzora_perf::DECLARATIONS)
        .function("version", |_| Ok("test".into()))
        .function("state", move |_| Ok(to_host(&read)))
        .async_function("tick", move |_| {
            let now = state.clone();
            Ok(async move { Ok(to_host(&now)) })
        })
}

#[gpui::test]
fn the_shipped_chip_draws_a_sample(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    let calls = Rc::new(RefCell::new(Calls::default()));
    let values = fixture(true, Value::Null, updates("current", "current"));
    let _ = crate::yuzora::with_modules(Policy::new(), "");
    let mut policy = Policy::new()
        .with_storage_path(staged.0.join("store-perf-place.json"))
        .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
        .with_host_module(module(values, calls.clone()))
        .unwrap();
    for m in crate::yuzora::host_modules("").into_iter().filter(|m| m.name() != yuzora_perf::NAME) {
        policy = policy.with_host_module(m).unwrap();
    }
    let policy = policy.with_host_module(one_sample()).unwrap();
    let window = cx.add_window(|_, _| Empty);
    let tree = window
        .update(cx, |_, window, cx| {
            let mut session = DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), staged.0.join("mods/yuzora-perf"), policy, window, cx).unwrap();
            session.describe(window, cx).unwrap();
            session.describe(window, cx).unwrap()
        })
        .unwrap();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
    assert!(calls.borrow().unexpected.is_empty(), "{:?}", calls.borrow().unexpected);
    // (A copy staged here has no strings folder, so it draws the string's key: "{cpu}% · {memory}MB" in en.json.)
    for part in ["text \"chip\"", "mods/yuzora-perf/cpu.svg", "mods/yuzora-perf/chevron-up.svg"] {
        assert!(tree.contains(part), "the chip draws {part}: {tree}");
    }
    assert!(!tree.contains("chip_waiting") && !tree.contains('⚠'), "a good sample, shown as one: {tree}");
    let strings: Value = serde_json::from_str(include_str!("../../../defaults/mods/yuzora-perf/i18n/en.json")).unwrap();
    assert_eq!(strings["chip"], "{cpu}% · {memory}MB");
}
