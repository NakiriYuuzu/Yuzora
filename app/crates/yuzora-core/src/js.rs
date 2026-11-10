//! `yuzora:core`, the module every Yuzora mod can import:
//!
//! ```js
//! import { t, tokens, panes, exec } from "yuzora:core";
//! ```
//!
//! One copy per mod copy (the app builds it with the mod's name), so `t(key)`
//! reads that mod's own strings.

use crate::host::{arg, error, from_host, opt_str, to_host, vars};
use gpui_shell::{HostModule, HostObject, HostValue};
use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const NAME: &str = "yuzora:core";

pub const DECLARATIONS: &str = r#"
export interface PaneEntry { mod: string; title: string; label: string; icon: string | null; order: number; props: unknown }
export interface SettingsEntry { mod: string; title: string; label: string; icon: string | null; order: number; section: string }
export interface ExecResult { code: number; stdout: string; stderr: string }
/** Yuzora's version. */
export function version(): string;
/** Yuzora's app home: ~/.config/yuzora, or $YUZORA_APP_HOME. */
export function appHome(): string;
/** One value for this run of the app, the same in every mod and different on the next run: tells a value kept in shared state (which survives a restart) from one this run wrote. */
export function session(): string;
/** The language shown: always "en". */
export function locale(): "en";
/** Does nothing: the interface is English only. Kept so older mods keep working. */
export function setLocale(choice: string): void;
/** This mod's text for key (mods/<mod>/i18n/en.json), {name} filled from vars. */
export function t(key: string, vars?: Record<string, string | number>): string;
/** Another mod's text for key. */
export function translate(mod: string, key: string, vars?: Record<string, string | number>): string;
/** Design tokens a theme has no name for: diff, agents, space_badges, status shapes, metrics, motion. For the theme showing: a light one gets its own text, diff and status colors. */
export function tokens(): Record<string, any>;
/** A named speed from tokens.json "motion", as `el.transition(property, policy)` takes it. A mod names one and writes no milliseconds; a name that is not one throws. */
export function motion(name: "fast" | "base"): { duration: number; easing: "linear" | "ease-in" | "ease-out" | "ease-in-out" };
/** Mods whose mod.json declares "yuzora": { "pane": {...} } (the + new pane menu). */
export function panes(): PaneEntry[];
/** Mods whose mod.json declares "yuzora": { "settings": {...} } (settings pages). */
export function settingsPages(): SettingsEntry[];
/** One section of Yuzora's settings.json (null when unset). */
export function getSetting(section: string): unknown;
/** Replaces one section of Yuzora's settings.json (null removes it). */
export function setSetting(section: string, value: unknown): void;
/** Runs a program here, or on a machine through hc exec --on, with its own timeout (default 30 s, 8 MiB). */
export function exec(program: string, args?: string[], options?: { cwd?: string; on?: string | null; timeoutMs?: number; maxBytes?: number }): Promise<ExecResult>;
"#;

static REFRESH: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// How Yuzora asks the app to redraw every mod (set once by the app).
pub fn set_refresh_hook(hook: impl Fn() + Send + Sync + 'static) {
    let _ = REFRESH.set(Box::new(hook));
}

pub fn request_refresh() {
    if let Some(hook) = REFRESH.get() {
        hook();
    }
}

/// One scan of the installed mods.
struct Scan {
    at: Instant,
    panes: Value,
    settings: Value,
}

/// Scans are cheap but renders are many: one scan serves them for a moment.
fn reuse_or_scan(last: &mut Option<Scan>, scan: impl FnOnce() -> (Value, Value)) -> (Value, Value) {
    if let Some(s) = last.as_ref()
        && s.at.elapsed() < Duration::from_secs(2)
    {
        return (s.panes.clone(), s.settings.clone());
    }
    let (panes, settings) = scan();
    *last = Some(Scan { at: Instant::now(), panes: panes.clone(), settings: settings.clone() });
    (panes, settings)
}

fn discovered() -> (Value, Value) {
    static LAST: Mutex<Option<Scan>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    reuse_or_scan(&mut last, || {
        let found = crate::discovery::scan_installed();
        (Value::Array(found.panes), Value::Array(found.settings))
    })
}

fn pairs(v: &[(String, String)]) -> Vec<(&str, String)> {
    v.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()
}

/// This run of the app: its process id and the time it was first asked.
fn session() -> String {
    static SESSION: OnceLock<String> = OnceLock::new();
    SESSION
        .get_or_init(|| {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            format!("{}-{nanos}", std::process::id())
        })
        .clone()
}

/// `yuzora:core` for one mod copy (`mod_name` "" outside a mod, e.g. `check`).
pub fn module(mod_name: &str) -> HostModule {
    crate::i18n::forget(mod_name);
    let own = mod_name.to_owned();
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(crate::VERSION.into()))
        .function("appHome", |_| Ok(crate::paths::app_home().to_string_lossy().into_owned().into()))
        .function("session", |_| Ok(session().into()))
        .function("locale", |_| Ok(crate::i18n::current().into()))
        // English only: older mods may still call this, and it must not fail.
        .function("setLocale", |_| Ok(HostValue::Null))
        .function("t", move |a| Ok(crate::i18n::translate(&own, a.string(0)?, &pairs(&vars(a, 1))).into()))
        .function("translate", |a| Ok(crate::i18n::translate(a.string(0)?, a.string(1)?, &pairs(&vars(a, 2))).into()))
        .function("tokens", |_| Ok(to_host(&crate::tokens::tokens())))
        .function("motion", |a| {
            let name = a.string(0)?;
            let all = crate::tokens::motion();
            match all.get(name) {
                Some(speed) => Ok(to_host(speed)),
                None => {
                    let known: Vec<_> = all.as_object().into_iter().flat_map(|o| o.keys().map(String::as_str)).collect();
                    Err(error(format!("no motion named {name:?}; the names are {}", known.join(", "))))
                }
            }
        })
        .function("panes", |_| Ok(to_host(&discovered().0)))
        .function("settingsPages", |_| Ok(to_host(&discovered().1)))
        .function("getSetting", |a| Ok(to_host(&crate::settings::section(a.string(0)?))))
        .function("setSetting", |a| {
            let section = a.string(0)?;
            crate::settings::set_section(section, arg(a, 1)).map_err(error)?;
            request_refresh();
            Ok(HostValue::Null)
        })
        .async_function("exec", |a| {
            let program = a.string(0)?.to_owned();
            let args: Vec<String> = match arg(a, 1) {
                Value::Array(l) => l.into_iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect(),
                _ => Vec::new(),
            };
            let opts = a.get(2).map(from_host).unwrap_or(Value::Null);
            let mut req = crate::remote::ExecRequest::new(program, args).cwd(opt_str(a, 2, "cwd")).on(opt_str(a, 2, "on"));
            if let Some(ms) = opts.get("timeoutMs").and_then(Value::as_f64).filter(|ms| *ms > 0.0) {
                req = req.timeout(Duration::from_millis(ms as u64));
            }
            if let Some(bytes) = opts.get("maxBytes").and_then(Value::as_f64).filter(|b| *b > 0.0) {
                req = req.max_output(bytes as u64);
            }
            let (tx, rx) = async_channel::bounded(1);
            std::thread::Builder::new()
                .name("yuzora-exec".into())
                .spawn(move || {
                    let _ = tx.send_blocking(crate::remote::run(&req));
                })
                .map_err(error)?;
            Ok(async move {
                let out = rx.recv().await.map_err(|_| error("the command was lost"))?.map_err(error)?;
                Ok(HostObject::new().field("code", out.code as f64).field("stdout", out.stdout_text()).field("stderr", out.stderr_text()).into())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_shell::HostArguments;
    use serde_json::json;

    #[test]
    fn the_module_validates_and_answers() {
        let m = module("yuzora-test-mod");
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:core");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(crate::VERSION.into()));
        // No strings for this mod: a key shows as itself, with its variables filled.
        let t = m.call("t", &HostArguments::new([HostValue::Str("hello {who}".into()), to_host(&json!({ "who": "Yuzora" }))])).unwrap();
        assert_eq!(t, HostValue::Str("hello Yuzora".into()));
        // English only: setLocale takes any old argument, changes nothing and does not fail.
        for choice in ["system", "en", "zh-TW", "fr"] {
            assert_eq!(m.call("setLocale", &HostArguments::new([HostValue::Str(choice.into())])).unwrap(), HostValue::Null, "{choice}");
            assert_eq!(m.call("locale", &HostArguments::new([])).unwrap(), HostValue::Str("en".into()), "after {choice}");
        }
        // One value for the run, whichever copy asks.
        let first = m.call("session", &HostArguments::new([])).unwrap();
        let again = module("another-mod").call("session", &HostArguments::new([])).unwrap();
        assert!(matches!(&first, HostValue::Str(s) if !s.is_empty()), "{first:?}");
        assert_eq!(first, again);
        assert!(m.is_async("exec"));
        let tokens = from_host(&m.call("tokens", &HostArguments::new([])).unwrap());
        assert_eq!(tokens["agents"]["claude"], "#c96a4b");
    }

    // `motion(name)` is what `el.transition(property, policy)` takes: a mod names a speed and writes no milliseconds.
    #[test]
    fn motion_hands_a_named_speed_to_transition() {
        let m = module("yuzora-test-mod");
        let ask = |name: &str| m.call("motion", &HostArguments::new([HostValue::Str(name.into())]));
        assert_eq!(from_host(&ask("fast").unwrap()), json!({ "duration": 120, "easing": "ease-out" }));
        assert_eq!(from_host(&ask("base").unwrap()), json!({ "duration": 160, "easing": "ease-out" }));
        // A typo is an error where it is written, not an animation of some default length.
        let wrong = format!("{:?}", ask("slow").unwrap_err());
        assert!(wrong.contains("slow") && wrong.contains("fast") && wrong.contains("base"), "{wrong}");
        // The names a mod can write are the tokens' names.
        let tokens = crate::tokens::tokens();
        for name in tokens["motion"].as_object().unwrap().keys() {
            assert!(DECLARATIONS.contains(&format!("\"{name}\"")), "{name} is not declared");
        }
    }
}
