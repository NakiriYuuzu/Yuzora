//! The resource chip in every layout (src/yuzora/dock.rs `place_perf`): each shipped layout, as the window shows it,
//! names the chip once, and the shipped `yuzora-perf` draws it with that layout as `layout()`.
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

/// What the copy of `yuzora-perf` draws in a window showing `layout`.
fn chip_in(cx: &mut TestAppContext, staged: &Staged, layout: &Value) -> String {
    let calls = Rc::new(RefCell::new(Calls::default()));
    let mut values = fixture(true, Value::Null, updates("current", "current"));
    values.insert("layout".into(), layout.clone());
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
    tree
}

#[gpui::test]
fn every_shipped_layout_draws_the_resource_chip(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    for (name, text) in [
        ("main", include_str!("../../../defaults/layouts/main.json")),
        ("terminals", include_str!("../../../defaults/layouts/terminals.json")),
        ("win98", include_str!("../../../defaults/layouts/win98.json")),
    ] {
        let layout = dock_layout(text);
        let chips = layout.to_string().matches(r#""mod":"yuzora-perf""#).count();
        assert_eq!(chips, 1, "{name} names the chip once: {layout}");
        // (A copy staged here has no strings folder, so it draws the string's key: "{cpu}% · {memory}MB" in en.json.)
        let tree = chip_in(cx, &staged, &layout);
        for part in ["text \"chip\"", "mods/yuzora-perf/cpu.svg", "mods/yuzora-perf/chevron-up.svg"] {
            assert!(tree.contains(part), "{name}: the chip draws {part}: {tree}");
        }
        assert!(!tree.contains("chip_waiting") && !tree.contains('⚠'), "{name}: a good sample, shown as one: {tree}");
    }
    let strings: Value = serde_json::from_str(include_str!("../../../defaults/mods/yuzora-perf/i18n/en.json")).unwrap();
    assert_eq!(strings["chip"], "{cpu}% · {memory}MB");
}
