//! The grid driven the way a person drives it: a window, the mouse, the keys,
//! and a pane of the width the Database dock leaves it.
//!
//! SQLite only (the app home is process-wide, so the tests share one).

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Bounds, Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement as _, Pixels, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use serde_json::json;
use yuzora_db::config::{self, ConnectionConfig};
use yuzora_db::model::{Cell, Column, ObjectRef};
use yuzora_db::results::{self, EditTarget, Editing, ResultSet, WriteState, WriteStop};
use yuzora_db::session::{self, JobState};
use yuzora_db::{grid, runtime};

static HOME: LazyLock<PathBuf> = LazyLock::new(|| {
    let home = std::env::temp_dir().join(format!("yuzora-db-grid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: first thing every test does, before it starts a thread of its own.
    unsafe {
        std::env::set_var("YUZORA_APP_HOME", &home);
        std::env::set_var("YUZORA_DB_SECRETS", "memory");
    }
    home
});

fn wait(job: session::JobId) -> JobState {
    let started = Instant::now();
    let mut seen = 0;
    while started.elapsed() < Duration::from_secs(30) {
        let state = runtime::block_on(session::job_after(job, seen)).expect("job");
        if !state.running {
            return state;
        }
        seen = state.version;
    }
    panic!("job {job} still running after 30 s");
}

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn rows(connection: &str, sql: &str) -> Vec<Vec<String>> {
    let state = wait(session::start_query(connection, None, &format!("check@{connection}"), sql));
    assert_eq!(state.error, None, "{sql}");
    let set = results::get(state.results.last().expect("a result").id).expect("result");
    let r = results::lock(&set);
    r.rows.iter().map(|row| row.iter().map(|c| c.display().into_owned()).collect()).collect()
}

static TURN: Mutex<()> = Mutex::new(());

/// A SQLite file with `setup` run in it, saved as a connection.
fn sqlite(name: &str, setup: &str) -> String {
    LazyLock::force(&HOME);
    let file = HOME.join(format!("{name}.sqlite"));
    let _ = std::fs::remove_file(&file);
    std::fs::File::create(&file).unwrap();
    let id = {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let config: ConnectionConfig = serde_json::from_value(json!({"name": name, "engine": "sqlite", "path": file})).unwrap();
        config::save(config, None).unwrap().id
    };
    let state = wait(session::start_query(&id, None, &format!("setup@{id}"), setup));
    assert_eq!(state.error, None, "{setup}");
    id
}

fn table(name: &str) -> ObjectRef {
    serde_json::from_value(json!({"schema": "", "name": name})).unwrap()
}

// ---------------------------------------------------------------- a window with a grid in it

/// A pane of a fixed size with one grid in it.
struct Pane {
    id: String,
    result: u64,
    width: f32,
    height: f32,
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "pane".into())
            .w(px(self.width))
            .h(px(self.height))
            .child(grid::element(&self.id, &json!({ "result": self.result }), window, cx))
    }
}

fn window<'a>(cx: &'a mut TestAppContext, id: &str, result: u64, width: f32, height: f32) -> &'a mut VisualTestContext {
    // A write-back runs on the database runtime's own threads and wakes the grid's
    // watcher task from there. gpui's test scheduler calls that nondeterminism
    // unless the test says it is mixing real I/O in.
    cx.executor().allow_parking();
    cx.update(gpui_base::init);
    let id = id.to_owned();
    let (_, cx) = cx.add_window_view(move |_, _| Pane { id, result, width, height });
    settle(cx);
    cx
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }
}

fn center(cx: &mut VisualTestContext, selector: &'static str) -> gpui::Point<Pixels> {
    let b = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
    b.center()
}

fn double_click(cx: &mut VisualTestContext, selector: &'static str) {
    let position = center(cx, selector);
    cx.simulate_event(MouseDownEvent { position, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: 2, first_mouse: false });
    cx.simulate_event(MouseUpEvent { position, modifiers: Modifiers::default(), button: MouseButton::Left, click_count: 2 });
    settle(cx);
}

fn save_keys() -> &'static str {
    if cfg!(target_os = "macos") { "cmd-s" } else { "ctrl-s" }
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let slack = px(0.5);
    inner.origin.x + slack >= outer.origin.x
        && inner.origin.y + slack >= outer.origin.y
        && inner.right() <= outer.right() + slack
        && inner.bottom() <= outer.bottom() + slack
}

// ---------------------------------------------------------------- ⌘S in the cell editor

fn open_people(name: &str, setup: &str, t: &str) -> (String, u64, Arc<Mutex<ResultSet>>) {
    let connection = sqlite(name, setup);
    let state = wait(session::open_table(&connection, None, &format!("{name}-pane"), &table(t), None));
    assert_eq!(state.error, None);
    let summary = state.results.first().expect("a result").clone();
    assert!(summary.editable, "{summary:?}");
    (connection, summary.id, results::get(summary.id).expect("result"))
}

/// Double-click, type, then `finish` the edit: the cell reads `x` in the database.
fn typed_edit_saves_with(cx: &mut TestAppContext, name: &'static str, finish: &str) {
    let (connection, id, _) = open_people(name, "create table people(id integer primary key, name text); insert into people values (1, 'a');", "people");
    let cx = window(cx, name, id, 600.0, 300.0);
    double_click(cx, "grid-cell-0-1");
    cx.simulate_keystrokes("x");
    settle(cx);
    cx.simulate_keystrokes(finish);
    settle(cx);
    until("the edit never reached the database", || rows(&connection, "select name from people") == [["x"]]);
}

#[gpui::test]
fn enter_then_save_writes_the_typed_cell(cx: &mut TestAppContext) {
    typed_edit_saves_with(cx, "grid-enter-save", &format!("enter {}", save_keys()));
}

#[gpui::test]
fn save_while_the_editor_is_still_open_writes_the_typed_cell(cx: &mut TestAppContext) {
    typed_edit_saves_with(cx, "grid-open-save", save_keys());
}

#[gpui::test]
fn a_value_the_database_refuses_shows_why_when_saved_from_the_editor(cx: &mut TestAppContext) {
    let (connection, id, set) = open_people(
        "grid-refused",
        "create table counts(id integer primary key, n integer) strict; insert into counts values (1, 5);",
        "counts",
    );
    let cx = window(cx, "grid-refused", id, 600.0, 300.0);
    double_click(cx, "grid-cell-0-1");
    cx.simulate_keystrokes("x");
    settle(cx);
    cx.simulate_keystrokes(save_keys());
    until("the refusal never showed", || results::lock(&set).write_failure().is_some());
    until("the reason never reached the screen", || {
        settle(cx);
        cx.debug_bounds("grid-message-open").is_some()
    });
    let r = results::lock(&set);
    assert!(r.write_failure().is_some_and(|m| m.contains("INTEGER")), "{:?}", r.write);
    assert_eq!(r.edits.len(), 1, "the edit stays pending");
    drop(r);
    assert_eq!(rows(&connection, "select n from counts"), [["5"]]);
}

// ---------------------------------------------------------------- a narrow pane

fn pending(owner: &str, state: impl FnOnce(&mut ResultSet)) -> u64 {
    let set = results::create(owner, "select * from t");
    let mut r = results::lock(&set);
    r.columns = vec![Column::new("id", "int4"), Column::new("name", "text")];
    r.rows = (0..20).map(|i| vec![Cell::text(i.to_string()), Cell::text("a")]).collect();
    r.complete = true;
    r.elapsed_ms = 18;
    r.editing = Editing::Editable(Box::new(EditTarget {
        connection: "c".into(),
        config: serde_json::from_value(json!({"name": "c", "engine": "sqlite", "path": "t.sqlite"})).unwrap(),
        database: None,
        owner: owner.into(),
        table: table("t"),
        key: vec![0],
        read_only: Vec::new(),
    }));
    r.set_edit(0, 1, Cell::text("edited"));
    state(&mut r);
    r.id
}

const LONG: &str = "duplicate key value violates unique constraint \"people_name_key\" (23505)\nDETAIL: Key (name)=(edited) already exists.";

#[gpui::test]
fn the_footers_buttons_stay_inside_a_pane_of_any_width(cx: &mut TestAppContext) {
    cx.update(gpui_base::init);
    type State = Box<dyn Fn(&mut ResultSet)>;
    let states: [(&str, State, &[&'static str]); 3] = [
        ("edited", Box::new(|_| {}), &["grid-discard", "grid-write"]),
        ("failed", Box::new(|r| r.write = WriteState::failed(LONG)), &["grid-discard", "grid-write", "grid-message"]),
        ("writing", Box::new(|r| r.write = WriteState::Writing(WriteStop::new())), &["grid-stop", "grid-discard"]),
    ];
    for width in [120.0, 150.0, 180.0, 240.0, 420.0] {
        for (name, state, buttons) in &states {
            let owner = format!("narrow-{name}-{width}");
            let id = pending(&owner, |r| state(r));
            let cx = window(cx, &owner, id, width, 300.0);
            let pane = cx.debug_bounds("pane").expect("the pane");
            for selector in *buttons {
                let b = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{name} at {width}px: {selector} is not drawn"));
                assert!(inside(b, pane), "{name} at {width}px: {selector} is at {b:?}, outside the pane {pane:?}");
                assert!(b.size.width >= px(40.0), "{name} at {width}px: {selector} is squeezed to {:?}", b.size);
            }
        }
    }
}

#[gpui::test]
fn a_failure_is_readable_in_a_narrow_pane(cx: &mut TestAppContext) {
    let owner = "narrow-failure";
    let id = pending(owner, |r| r.write = WriteState::failed(LONG));
    let cx = window(cx, owner, id, 150.0, 300.0);
    let pane = cx.debug_bounds("pane").expect("the pane");
    let open = cx.debug_bounds("grid-message-open").expect("a new failure opens by itself");
    assert!(inside(open, pane), "{open:?} outside {pane:?}");
    assert!(open.size.width >= px(120.0), "the text is wrapped into {:?}", open.size);
    assert!(open.size.height > px(40.0), "the whole message is there, wrapped over lines: {:?}", open.size);
}
