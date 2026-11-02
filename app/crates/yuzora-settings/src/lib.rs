//! Yuzora L6（設定＋MOD／Template）: Settings and extensions: the settings frame, mods and herdr-core templates management.
//!
//! Owned by lane L6. Scripts reach it as `import { … } from "yuzora:settings"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! - [`templates`]: herdr-core agent templates, read and written as hc does,
//!   here or on another machine (`hc exec --on`).
//! - [`launch`]: starting an agent from a template into a space and tab.
//! - [`mods`]: the home's mods and layouts, shipped or not; installing a folder.
//! - [`trash`]: the system trash.
//! - [`startup`]: what `general.startup` asks of the window when it opens.

pub mod launch;
pub mod mods;
pub mod templates;
pub mod trash;

use serde_json::{Value, json};
use std::time::Duration;
use yuzora_core::host::{arg, error, from_host, to_host};
use yuzora_core::remote::{ExecRequest, run};
use yuzora_core::{HostArguments, HostError, HostModule, HostValue};

/// The import specifier: `yuzora:settings`.
pub const NAME: &str = "yuzora:settings";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export interface TemplateFields { command: string; description: string; ready: string; submit: string; stop: string; paste_gap_ms: string }
export interface TemplateIssue { field: string; code: string; level: "error" | "warning" }
export interface TemplateEntry {
  name: string; path: string; text: string; fields: TemplateFields; others: [string, string][]; body: string;
  /** hc's verdict: why it won't run (no_frontmatter, never_closes, not_key_value, no_command, paste_gap_number). */
  error: { code: string; detail: string | null } | null;
  issues: TemplateIssue[];
}
export interface TemplateList { machine: string | null; dir: string | null; templates: TemplateEntry[]; error: string | null }
export interface Outcome { ok: boolean; code?: string; message?: string; [k: string]: unknown }
export interface Launch { template: string; task: string; machine?: string | null; space: string; tab?: string | null; cwd?: string | null; name?: string | null }
export interface ModEntry {
  name: string; path: string; title: string | null; place: string | null; main: boolean; manifest: boolean;
  shipped: boolean; edited: boolean; resettable: boolean; cloned_from: { name: string; version: string } | null;
  pane: boolean; settings: boolean; source: "shipped" | "edited" | "clone" | "personal";
}
export interface LayoutEntry { name: string; path: string; valid: boolean; shipped: boolean; cloned_from: { name: string; version: string } | null; preview: string | null; source: "shipped" | "clone" | "personal" }
export interface InstallPlan { kind: "mod" | "bundle"; source: string; mods: { from: string; as: string }[]; layouts: { from: string; as: string }[]; keys: boolean; reaches: ("programs" | "network" | "files")[]; files: number; bytes: number; token: string }
/** The yuzora-settings crate's version (Yuzora's). */
export function version(): string;
/** herdr-core's templates folder on this computer. */
export function templatesDir(): string;
/** Every template on a machine (null: this computer), read from its folder as hc reads it. */
export function listTemplates(machine?: string | null): Promise<TemplateList>;
/** What stops a template from saving (level error) or may not do what was meant (warning). */
export function checkTemplate(name: string, fields: TemplateFields, body: string, machine?: string | null): TemplateIssue[];
/** The file a template saves as: the fields, the header lines kept from the file (others), the body. */
export function templateText(fields: TemplateFields, others: [string, string][], body: string): string;
/** Saves a template (expected null: a new one; else the text it was read as). previous: its old name, when renamed (that file goes to the trash). Resolves {ok, text} or {ok: false, code: exists | changed | invalid | name_invalid | old_kept (renamed; the old file is still there) | too_big (over 12 KiB, on another machine) | io | remote, message}. */
export function saveTemplate(machine: string | null, name: string, fields: TemplateFields, others: [string, string][], body: string, expected: string | null, previous?: string | null): Promise<Outcome>;
/** Moves a template to the trash. */
export function trashTemplate(machine: string | null, name: string): Promise<Outcome>;
/** Starts an agent from a template into a space and tab (tab null: a new tab). Resolves {ok, address, sent, reason} or {ok: false, kind, message}. */
export function runTemplate(launch: Launch): Promise<Outcome>;
/** The home's mods and layouts. */
export function listMods(): Promise<{ mods: ModEntry[]; layouts: LayoutEntry[] }>;
/** Puts a shipped mod back as shipped; an edited one first moves to <name>-edited (returned). */
export function resetMod(name: string): string | null;
/** The window's `status` (layout, overlays, errors), as `herdr-app status` prints it. */
export function appStatus(): Promise<Outcome>;
/** `herdr-app check` on one installed mod: {ok, output}. */
export function checkMod(name: string): Promise<Outcome>;
/** What installing a folder would do (nothing is written). */
export function planInstall(path: string): Promise<Outcome & { plan?: InstallPlan }>;
/** Installs the files planInstall read (plan.token: the latest plan), never overwriting: what was shown is what's installed, whatever the folder holds now. code stale: plan again. */
export function installFolder(token: string): Promise<Outcome & { plan?: InstallPlan }>;
/** Opens a folder (or shows a file) in the system's file manager. */
export function reveal(path: string): null;
/** general.startup: the sidebar mode to show when the window opens (a yuzora-sidebar child), or null for the last one. */
export function startup(): { sidebar: string | null };
/** This run of the app: a different value after every start (for shared values that mean nothing after a restart, like an overlay id). */
export function session(): string;
"#;

/// Remote template work is a few small files: a short wait is plenty.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
/// A template run waits for the agent's first status report (hc: 30 s by default).
const RUN_TIMEOUT: Duration = Duration::from_secs(90);

/// This process and when it started: overlay ids restart at o1 with the app,
/// so one kept in `shared` across a restart may name someone else's overlay.
fn session() -> &'static str {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION.get_or_init(|| {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{}-{nanos}", std::process::id())
    })
}

/// Runs blocking work on its own thread; the script gets a promise.
fn background<F>(work: F) -> Result<impl std::future::Future<Output = Result<HostValue, HostError>> + Send + 'static, HostError>
where
    F: FnOnce() -> Value + Send + 'static,
{
    let (tx, rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("yuzora-settings".into())
        .spawn(move || {
            let _ = tx.send_blocking(work());
        })
        .map_err(error)?;
    Ok(async move { Ok(to_host(&rx.recv().await.map_err(|_| error("the work was lost"))?)) })
}

fn machine_arg(a: &HostArguments, i: usize) -> Option<String> {
    arg(a, i).as_str().map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned)
}

fn failed(code: &str, message: impl std::fmt::Display) -> Value {
    json!({ "ok": false, "code": code, "message": message.to_string() })
}

fn sh(machine: &str, script: &str, args: &[&str]) -> Result<yuzora_core::remote::ExecOutput, Value> {
    let mut all = vec!["-c", script, "sh"];
    all.extend_from_slice(args);
    run(&ExecRequest::new("sh", all).on(Some(machine)).timeout(REMOTE_TIMEOUT)).map_err(|e| failed("remote", e))
}

fn list_templates(machine: Option<String>) -> Value {
    let (dir, listed) = match &machine {
        None => {
            let dir = templates::dir();
            let listed = templates::list(&dir).map_err(|e| e.to_string());
            (Some(dir.to_string_lossy().into_owned()), listed)
        }
        Some(m) => match sh(m, templates::REMOTE_LIST, &[]) {
            Ok(out) if out.code == 0 => match templates::parse_remote_list(&out.stdout_text()) {
                Some((dir, entries)) => (Some(dir), Ok(entries)),
                None => (None, Err("the list was cut short: more than exec's 8 MiB of templates".to_owned())),
            },
            Ok(out) => (None, Err(out.stderr_text().trim().to_owned())),
            Err(e) => (None, Err(e["message"].as_str().unwrap_or_default().to_owned())),
        },
    };
    let (entries, err) = match listed {
        Ok(e) => (e, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    json!({ "machine": machine, "dir": dir, "templates": entries.iter().map(templates::Entry::to_json).collect::<Vec<_>>(), "error": err })
}

fn others_arg(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get(0)?.as_str()?.trim().to_owned(), p.get(1)?.as_str()?.trim().to_owned())))
        .filter(|(k, v)| !k.is_empty() && !k.contains([':', '\n', '\r']) && !v.contains(['\n', '\r']) && k != "---")
        .collect()
}

struct Save {
    machine: Option<String>,
    name: String,
    text: String,
    expected: Option<String>,
    previous: Option<String>,
}

fn save_template(s: Save) -> Value {
    // Names go into paths on the other machine too: checked here for both.
    if !templates::valid_name(&s.name) || s.previous.as_deref().is_some_and(|p| !templates::valid_name(p)) {
        return failed("name_invalid", &s.name);
    }
    match &s.machine {
        None => match templates::save(&templates::dir(), &s.name, &s.text, s.expected.as_deref(), s.previous.as_deref()) {
            Ok(()) => json!({ "ok": true, "text": s.text }),
            Err(e) => failed(e.code(), e),
        },
        Some(m) => {
            // The text goes as an argument: past REMOTE_MAX the command line
            // wouldn't start. The text it was read as goes as its cksum.
            if s.text.len() > templates::REMOTE_MAX {
                return failed("too_big", templates::REMOTE_MAX);
            }
            let mode = if s.expected.is_some() { "old" } else { "new" };
            let sum = s.expected.as_deref().map(|e| templates::cksum(e.as_bytes())).unwrap_or_default();
            let args = [s.name.as_str(), s.text.as_str(), mode, sum.as_str(), s.previous.as_deref().unwrap_or("")];
            match sh(m, &templates::remote_save_script(), &args) {
                Ok(out) => match out.code {
                    0 => json!({ "ok": true, "text": s.text }),
                    4 => failed("exists", "exists"),
                    5 => failed("changed", "changed"),
                    7 => failed("old_kept", out.stderr_text().trim()),
                    _ => failed("io", out.stderr_text().trim()),
                },
                Err(e) => e,
            }
        }
    }
}

fn trash_template(machine: Option<String>, name: String) -> Value {
    if !templates::valid_name(&name) {
        return failed("name_invalid", &name);
    }
    match &machine {
        None => match templates::trash(&templates::dir(), &name) {
            Ok(()) => json!({ "ok": true }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => failed("missing", e),
            Err(e) => failed("io", e),
        },
        Some(m) => match sh(m, &templates::remote_trash_script(), &[&name]) {
            Ok(out) if out.code == 0 => json!({ "ok": true }),
            Ok(out) if out.code == 3 => failed("missing", "missing"),
            Ok(out) => failed("io", out.stderr_text().trim()),
            Err(e) => e,
        },
    }
}

fn launch_from(v: &Value) -> launch::Launch {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    launch::Launch {
        template: s("template").unwrap_or_default(),
        task: v.get("task").and_then(Value::as_str).unwrap_or_default().to_owned(),
        machine: s("machine"),
        space: s("space").unwrap_or_default(),
        tab: s("tab"),
        cwd: s("cwd"),
        name: s("name"),
    }
}

fn run_template(mut l: launch::Launch) -> Value {
    if let Some(code) = launch::check(&l) {
        return failed(code, code);
    }
    // Only a template that hc would run: an unknown name would start a program.
    let listed = list_templates(l.machine.clone());
    let found = listed["templates"].as_array().and_then(|t| t.iter().find(|e| e["name"] == l.template.as_str())).cloned();
    match found {
        None if listed["error"].is_string() => return failed("remote", listed["error"].as_str().unwrap_or_default()),
        None => return failed("template_missing", &l.template),
        Some(e) if !e["error"].is_null() => return failed("template_broken", e["error"]["code"].as_str().unwrap_or_default()),
        Some(_) => {}
    }
    if l.machine.is_none() && l.cwd.is_none() {
        // hc's default is the folder hc runs in: the app's, not the person's.
        l.cwd = Some(yuzora_core::paths::home_dir().to_string_lossy().into_owned());
    }
    let tab = l.tab.clone().unwrap_or_else(launch::new_tab_id);
    let req = ExecRequest::new(yuzora_core::remote::hc_program().to_string_lossy().into_owned(), launch::args(&l, &tab)).timeout(RUN_TIMEOUT);
    match run(&req) {
        Ok(out) => {
            let mut v = launch::parse_result(out.code, &out.stdout_text(), &out.stderr_text());
            v["tab"] = json!(tab);
            v
        }
        Err(e) => failed("failed", e),
    }
}

/// This app's own binary: `status` and `check` are its subcommands.
fn app_binary() -> Result<String, Value> {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).map_err(|e| failed("io", e))
}

fn app_status() -> Value {
    let bin = match app_binary() {
        Ok(b) => b,
        Err(e) => return e,
    };
    match run(&ExecRequest::new(bin, ["status"]).timeout(Duration::from_secs(10))) {
        Ok(out) if out.code == 0 => match serde_json::from_str::<Value>(out.stdout_text().trim()) {
            Ok(Value::Object(mut m)) => {
                m.insert("ok".into(), json!(true));
                Value::Object(m)
            }
            _ => failed("protocol", out.stdout_text().trim()),
        },
        Ok(out) => failed("failed", format!("{}{}", out.stdout_text(), out.stderr_text()).trim()),
        Err(e) => failed("failed", e),
    }
}

fn check_mod(name: String) -> Value {
    if !mods::valid_name(&name) {
        return failed("name_invalid", &name);
    }
    let dir = yuzora_core::paths::mods_dir().join(&name);
    if !dir.is_dir() {
        return failed("missing", &name);
    }
    let bin = match app_binary() {
        Ok(b) => b,
        Err(e) => return e,
    };
    match run(&ExecRequest::new(bin, ["check".to_owned(), dir.to_string_lossy().into_owned()]).timeout(Duration::from_secs(90))) {
        Ok(out) => json!({ "ok": out.code == 0, "output": format!("{}{}", out.stdout_text(), out.stderr_text()).trim() }),
        Err(e) => failed("failed", e),
    }
}

fn install_outcome(r: Result<mods::Plan, mods::InstallError>) -> Value {
    match r {
        Ok(p) => json!({ "ok": true, "plan": p.to_json() }),
        Err(e) => failed(e.code(), e),
    }
}

/// The latest install plan shown, with the files it read: installFolder
/// writes these, never the folder as it is by then.
static PLANNED: std::sync::Mutex<Option<(String, mods::Plan)>> = std::sync::Mutex::new(None);

fn plan_install(path: &str) -> Value {
    match mods::plan(std::path::Path::new(path), &yuzora_core::paths::app_home()) {
        Ok(plan) => {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let token = format!("{}-{}", session(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
            let mut shown = plan.to_json();
            shown["token"] = json!(token);
            *PLANNED.lock().unwrap_or_else(|e| e.into_inner()) = Some((token, plan));
            json!({ "ok": true, "plan": shown })
        }
        Err(e) => failed(e.code(), e),
    }
}

fn install_planned(token: &str) -> Value {
    let planned = {
        let mut slot = PLANNED.lock().unwrap_or_else(|e| e.into_inner());
        match slot.take() {
            Some((t, plan)) if t == token => plan,
            other => {
                *slot = other;
                return failed("stale", "plan again");
            }
        }
    };
    install_outcome(mods::write(&planned, &yuzora_core::paths::app_home()).map(|()| planned))
}

fn reveal(path: &str) -> Result<(), HostError> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(error(format!("{path} doesn't exist")));
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("/usr/bin/open");
        if p.is_file() {
            c.arg("-R");
        }
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        if p.is_file() {
            c.arg("/select,");
        }
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let target = if !cfg!(any(target_os = "macos", windows)) && p.is_file() { p.parent().unwrap_or(p) } else { p };
    let mut child = cmd.arg(target).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(error)?;
    // Reap it off the UI thread; the file manager itself outlives it.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// What `general.startup` asks for when the window opens.
pub mod startup {
    use serde_json::Value;

    pub const MODES: &[&str] = &["workspace", "files", "git", "database", "ssh"];

    /// `general.startup.sidebar`: a mode (`git`) shows that sidebar child
    /// (`yuzora-sidebar-git`); `"last"` or unset leaves the layout's own.
    pub fn sidebar(general: &Value) -> Option<String> {
        let mode = general.pointer("/startup/sidebar").and_then(Value::as_str)?;
        MODES.contains(&mode).then(|| format!("yuzora-sidebar-{mode}"))
    }
}

/// The `yuzora:settings` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .function("templatesDir", |_| Ok(templates::dir().to_string_lossy().into_owned().into()))
        .async_function("listTemplates", |a| {
            let machine = machine_arg(a, 0);
            background(move || list_templates(machine))
        })
        .function("checkTemplate", |a| {
            // Saved on this computer (no machine): its own file name rules too (Windows).
            let check = if machine_arg(a, 3).is_none() { templates::check_here } else { templates::check };
            let issues = check(a.string(0)?, &arg(a, 1), a.string(2).unwrap_or_default());
            Ok(to_host(&Value::Array(issues.iter().map(templates::Issue::to_json).collect())))
        })
        .function("templateText", |a| Ok(templates::render(&arg(a, 0), &others_arg(&arg(a, 1)), a.string(2).unwrap_or_default()).into()))
        .async_function("saveTemplate", |a| {
            let machine = machine_arg(a, 0);
            let name = a.string(1)?.trim().to_owned();
            let (fields, others, body) = (arg(a, 2), others_arg(&arg(a, 3)), a.string(4).unwrap_or_default().to_owned());
            let issues = templates::check(&name, &fields, &body);
            let blocked = issues.iter().find(|i| i.error).map(|i| format!("{}: {}", i.field, i.code));
            let save = Save {
                machine,
                text: templates::render(&fields, &others, &body),
                name,
                expected: arg(a, 5).as_str().map(str::to_owned),
                previous: machine_arg(a, 6),
            };
            background(move || match blocked {
                Some(why) => failed("invalid", why),
                None => save_template(save),
            })
        })
        .async_function("trashTemplate", |a| {
            let (machine, name) = (machine_arg(a, 0), a.string(1)?.to_owned());
            background(move || trash_template(machine, name))
        })
        .async_function("runTemplate", |a| {
            let l = launch_from(&arg(a, 0));
            background(move || run_template(l))
        })
        .async_function("listMods", |_| {
            background(|| {
                let home = yuzora_core::paths::app_home();
                json!({ "mods": mods::list_mods(&home), "layouts": mods::list_layouts(&home) })
            })
        })
        .function("resetMod", |a| {
            let name = a.string(0)?;
            if !mods::valid_name(name) {
                return Err(error(format!("{name:?} is not a mod name")));
            }
            let kept = mods::reset_mod(&yuzora_core::paths::app_home(), name).map_err(error)?;
            Ok(kept.map_or(HostValue::Null, HostValue::Str))
        })
        .async_function("appStatus", |_| background(app_status))
        .async_function("checkMod", |a| {
            let name = a.string(0)?.to_owned();
            background(move || check_mod(name))
        })
        .async_function("planInstall", |a| {
            let path = a.string(0)?.to_owned();
            background(move || plan_install(&path))
        })
        .async_function("installFolder", |a| {
            let token = a.string(0)?.to_owned();
            background(move || install_planned(&token))
        })
        .function("reveal", |a| {
            reveal(a.string(0)?)?;
            Ok(HostValue::Null)
        })
        .function("startup", |_| {
            let general = yuzora_core::settings::section(yuzora_core::settings::GENERAL);
            Ok(to_host(&json!({ "sidebar": startup::sidebar(&general) })))
        })
        .function("session", |_| Ok(HostValue::Str(session().to_owned())))
}

/// Arguments as JSON, for tests and callers outside a script.
pub fn call(f: &str, args: Vec<Value>) -> Result<Value, HostError> {
    host_module().call(f, &HostArguments::new(args.iter().map(to_host))).map(|v| from_host(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:settings");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
        for f in ["listTemplates", "saveTemplate", "trashTemplate", "runTemplate", "listMods", "appStatus", "checkMod", "planInstall", "installFolder"] {
            assert!(m.is_async(f), "{f} must not block the UI thread");
        }
    }

    #[test]
    fn check_and_text_reach_the_script() {
        let issues = call("checkTemplate", vec![json!("a/b"), json!({ "command": "" }), json!("")]).unwrap();
        let codes: Vec<_> = issues.as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap().to_owned()).collect();
        assert_eq!(codes, ["name_invalid", "command_empty", "body_empty"]);
        let text = call("templateText", vec![json!({ "command": "pi" }), json!([["model", "x"], ["bad:key", "y"], ["k", "two\nlines"]]), json!("Hi")]).unwrap();
        assert_eq!(text, "---\ncommand: pi\nmodel: x\n---\nHi\n", "header lines that would break the file are dropped");
    }

    #[test]
    fn a_session_is_one_value_for_this_run() {
        let s = call("session", vec![]).unwrap();
        assert!(s.as_str().is_some_and(|s| s.starts_with(&format!("{}-", std::process::id()))));
        assert_eq!(call("session", vec![]).unwrap(), s);
    }

    #[test]
    fn startup_names_a_sidebar_child() {
        assert_eq!(startup::sidebar(&json!({ "startup": { "sidebar": "git" } })).as_deref(), Some("yuzora-sidebar-git"));
        assert_eq!(startup::sidebar(&json!({ "startup": { "sidebar": "last" } })), None);
        assert_eq!(startup::sidebar(&json!({ "startup": { "sidebar": "../x" } })), None);
        assert_eq!(startup::sidebar(&Value::Null), None);
    }

    #[test]
    fn reveal_refuses_a_missing_path() {
        assert!(call("reveal", vec![json!("/definitely/not/here/yuzora")]).is_err());
    }
}
