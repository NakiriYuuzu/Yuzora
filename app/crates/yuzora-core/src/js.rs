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
use serde_json::{Value, json};
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
/** The language shown: "en" or "zh-TW" (general.locale, "system" by default). */
export function locale(): "en" | "zh-TW";
/** Sets general.locale and redraws every mod. */
export function setLocale(choice: "system" | "en" | "zh-TW"): void;
/** This mod's text for key (mods/<mod>/i18n/<locale>.json, English beneath), {name} filled from vars. */
export function t(key: string, vars?: Record<string, string | number>): string;
/** Another mod's text for key. */
export function translate(mod: string, key: string, vars?: Record<string, string | number>): string;
/** Design tokens a theme has no name for: diff, agents, space_badges, status shapes, metrics. */
export function tokens(): Record<string, any>;
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

/// One scan of the installed mods, and the language its labels are in.
struct Scan {
    at: Instant,
    locale: &'static str,
    panes: Value,
    settings: Value,
}

/// Scans are cheap but renders are many: one scan serves them for a moment,
/// unless the language changed since (the labels are translated).
fn reuse_or_scan(last: &mut Option<Scan>, locale: &'static str, scan: impl FnOnce() -> (Value, Value)) -> (Value, Value) {
    if let Some(s) = last.as_ref()
        && s.locale == locale
        && s.at.elapsed() < Duration::from_secs(2)
    {
        return (s.panes.clone(), s.settings.clone());
    }
    let (panes, settings) = scan();
    *last = Some(Scan { at: Instant::now(), locale, panes: panes.clone(), settings: settings.clone() });
    (panes, settings)
}

fn discovered() -> (Value, Value) {
    static LAST: Mutex<Option<Scan>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    reuse_or_scan(&mut last, crate::i18n::current(), || {
        let found = crate::discovery::scan_installed();
        (Value::Array(found.panes), Value::Array(found.settings))
    })
}

fn pairs(v: &[(String, String)]) -> Vec<(&str, String)> {
    v.iter().map(|(k, v)| (k.as_str(), v.clone())).collect()
}

/// `yuzora:core` for one mod copy (`mod_name` "" outside a mod, e.g. `check`).
pub fn module(mod_name: &str) -> HostModule {
    crate::i18n::forget(mod_name);
    let own = mod_name.to_owned();
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(crate::VERSION.into()))
        .function("appHome", |_| Ok(crate::paths::app_home().to_string_lossy().into_owned().into()))
        .function("locale", |_| Ok(crate::i18n::current().into()))
        .function("setLocale", |a| {
            let choice = a.string(0)?;
            if !matches!(choice, "system" | "en" | "zh-TW") {
                return Err(error(format!("setLocale: {choice:?} is not system, en or zh-TW")));
            }
            let mut general = crate::settings::section(crate::settings::GENERAL);
            if !general.is_object() {
                general = json!({});
            }
            general["locale"] = json!(choice);
            crate::settings::set_section(crate::settings::GENERAL, general).map_err(error)?;
            crate::i18n::refresh();
            request_refresh();
            Ok(HostValue::Null)
        })
        .function("t", move |a| Ok(crate::i18n::translate(&own, a.string(0)?, &pairs(&vars(a, 1))).into()))
        .function("translate", |a| Ok(crate::i18n::translate(a.string(0)?, a.string(1)?, &pairs(&vars(a, 2))).into()))
        .function("tokens", |_| Ok(to_host(&crate::tokens::tokens())))
        .function("panes", |_| Ok(to_host(&discovered().0)))
        .function("settingsPages", |_| Ok(to_host(&discovered().1)))
        .function("getSetting", |a| Ok(to_host(&crate::settings::section(a.string(0)?))))
        .function("setSetting", |a| {
            let section = a.string(0)?;
            crate::settings::set_section(section, arg(a, 1)).map_err(error)?;
            // general.locale may have changed, as with setLocale.
            if section == crate::settings::GENERAL {
                crate::i18n::refresh();
            }
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

    #[test]
    fn the_module_validates_and_answers() {
        let m = module("yuzora-test-mod");
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:core");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(crate::VERSION.into()));
        // No strings for this mod: a key shows as itself, with its variables filled.
        let t = m.call("t", &HostArguments::new([HostValue::Str("hello {who}".into()), to_host(&json!({ "who": "Yuzora" }))])).unwrap();
        assert_eq!(t, HostValue::Str("hello Yuzora".into()));
        assert!(m.call("setLocale", &HostArguments::new([HostValue::Str("fr".into())])).is_err());
        assert!(m.is_async("exec"));
        let tokens = from_host(&m.call("tokens", &HostArguments::new([])).unwrap());
        assert_eq!(tokens["agents"]["claude"], "#c96a4b");
    }

    #[test]
    fn discovered_labels_are_scanned_again_in_a_new_language() {
        let mut last = None;
        let mut scans = 0;
        let mut scan = |locale: &'static str| {
            reuse_or_scan(&mut last, locale, || {
                scans += 1;
                (json!([locale]), json!([]))
            })
        };
        assert_eq!(scan("en").0, json!(["en"]));
        assert_eq!(scan("en").0, json!(["en"]));
        assert_eq!(scan("zh-TW").0, json!(["zh-TW"]));
        assert_eq!(scans, 2);
    }
}
