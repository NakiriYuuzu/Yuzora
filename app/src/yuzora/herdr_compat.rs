//! yuzora(L9): Yuzora and the official Herdr app can share one herdr-core
//! (docs/beta2/herdr-compat.md). What the person reads when the core can't
//! serve Yuzora: on the setup screen (setup.rs), and when the core that comes
//! back after a restart is one Yuzora can't use (window.rs reconnects).
//! Herdr can update or replace the shared core while Yuzora runs, so both
//! say the core is shared and name the right app. Strings: herdr_compat/*.json,
//! in Yuzora's language.

use crate::hc::{GENERATION, Mismatch};
use crate::setup::Need;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use yuzora_core::i18n::{self, Catalog};

const EN: &str = include_str!("herdr_compat/en.json");
const ZH_TW: &str = include_str!("herdr_compat/zh-TW.json");

fn catalog() -> Catalog {
    Catalog::from_json(EN, (i18n::current() == i18n::ZH_TW).then_some(ZH_TW))
}

/// Title, what's going on, and the line under the buttons (setup.rs `words`).
/// `place` and `install`: where and how the core's own installer puts it;
/// `app`: where to get a newer Yuzora.
pub fn setup_words(need: &Need, place: &str, install: &str, app: &str) -> (String, String, String) {
    setup_words_in(&catalog(), need, place, install, app)
}

fn setup_words_in(c: &Catalog, need: &Need, place: &str, install: &str, app: &str) -> (String, String, String) {
    let shared = ("shared", c.t("setup.shared", &[]));
    match need {
        Need::Install => (
            c.t("setup.install.title", &[]),
            c.t("setup.install.body", &[]),
            c.t("setup.install.foot", &[("place", place.into()), ("install", install.into())]),
        ),
        Need::Fix(Mismatch::TooOld { have, need }) => (
            c.t("setup.old.title", &[]),
            c.t("setup.old.body", &[("have", have.clone()), ("need", need.clone()), shared]),
            c.t("setup.update", &[]),
        ),
        Need::Fix(Mismatch::Missing(methods)) => (
            c.t("setup.missing.title", &[]),
            c.t("setup.missing.body", &[("methods", methods.join(", ")), shared]),
            c.t("setup.update", &[]),
        ),
        Need::Fix(Mismatch::Newer { generation }) => (
            c.t("setup.newer.title", &[]),
            c.t("setup.newer.body", &[("generation", generation.to_string()), ("ours", GENERATION.to_string())]),
            c.t("setup.newer.foot", &[("app", app.into())]),
        ),
    }
}

/// The overlay id of the toast that says why a core was refused: one at a
/// time, replaced by a newer mismatch, closed when a core serves again.
pub const REFUSED_TOAST: &str = "yuzora-core-refused";

/// How long the toast stays up; while the same mismatch lasts it's shown
/// again after that (or a day after the person clicked it away).
const TOAST_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// The mismatch of the refused core, and when its toast was shown.
pub struct Told {
    mismatch: Mismatch,
    shown: Instant,
}

/// Whether the core that answered a reconnect can serve Yuzora (the same
/// check as at start, `hc::check`). Ok(true): it serves, and the toast about
/// a refused one is up (close it). Err: refuse it; with the toast to show when
/// this mismatch is new or its toast has run out (`told` remembers it).
pub fn serves(hello: &Value, told: &mut Option<Told>) -> Result<bool, Option<Value>> {
    serves_in(&catalog(), hello, told, Instant::now())
}

fn serves_in(c: &Catalog, hello: &Value, told: &mut Option<Told>, now: Instant) -> Result<bool, Option<Value>> {
    let mismatch = match crate::hc::check(hello) {
        Ok(()) => return Ok(told.take().is_some()),
        Err(m) => m,
    };
    if told.as_ref().is_some_and(|t| t.mismatch == mismatch && now.duration_since(t.shown) < TOAST_FOR) {
        return Err(None);
    }
    // The toast shows one line of each: the title says what to update, the body why.
    let (title, body) = match &mismatch {
        Mismatch::TooOld { have, need } => (c.t("refused.old.title", &[]), c.t("refused.old.body", &[("have", have.clone()), ("need", need.clone())])),
        Mismatch::Missing(methods) => (c.t("refused.missing.title", &[]), c.t("refused.missing.body", &[("methods", methods.join(", "))])),
        Mismatch::Newer { generation } => (c.t("refused.newer.title", &[]), c.t("refused.newer.body", &[("generation", generation.to_string())])),
    };
    *told = Some(Told { mismatch, shown: now });
    // Up while Yuzora stays disconnected: it's why the panes wait.
    // A click opens Updates, where both the core and Yuzora are updated.
    let updates = json!({"name": "app.settings", "args": {"section": "updates"}});
    Err(Some(json!({"kind": "toast", "notification": {"title": title, "body": body, "level": "error", "timeout": TOAST_FOR.as_millis() as u64, "action": updates}})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogs() -> [Catalog; 2] {
        [Catalog::from_json(EN, None), Catalog::from_json(EN, Some(ZH_TW))]
    }

    const USES_BUT_ONE: &[&str] = &["terminal.run", "terminal.list", "terminal.kill", "terminal.label", "terminal.attach", "terminal.keep", "doc.get", "doc.set"];

    fn hello(generation: u64, version: &str) -> Value {
        json!({"generation": generation, "version": version, "methods": crate::hc::USES})
    }

    /// About how wide a line is in the toast: CJK and fullwidth count twice.
    fn width(text: &str) -> usize {
        text.chars().map(|c| if c as u32 >= 0x2E80 { 2 } else { 1 }).sum()
    }

    fn keys(json: &str) -> Vec<String> {
        fn walk(v: &Value, at: String, out: &mut Vec<String>) {
            match v.as_object() {
                Some(m) => m.iter().for_each(|(k, v)| walk(v, if at.is_empty() { k.clone() } else { format!("{at}.{k}") }, out)),
                None => out.push(at),
            }
        }
        let mut out = Vec::new();
        walk(&serde_json::from_str(json).unwrap(), String::new(), &mut out);
        out.sort();
        out
    }

    #[test]
    fn both_languages_have_every_string() {
        assert_eq!(keys(EN), keys(ZH_TW));
    }

    #[test]
    fn every_core_screen_says_yuzora_and_that_the_core_is_shared() {
        let needs = [
            Need::Install,
            Need::Fix(Mismatch::TooOld { have: "0.1.9".into(), need: "0.1.11".into() }),
            Need::Fix(Mismatch::Missing(vec!["doc.set".into()])),
            Need::Fix(Mismatch::Newer { generation: 2 }),
        ];
        for c in &catalogs() {
            for need in &needs {
                let (title, body, foot) = setup_words_in(c, need, "~/.local/bin", "sh install.sh", "https://releases");
                let all = format!("{title} {body} {foot}");
                assert!(title.contains("Yuzora"), "{title}");
                assert!(!all.contains('{') && !all.contains("this Herdr") && !all.contains("Herdr needs") && !all.contains("Update Herdr"), "{all}");
                if !matches!(need, Need::Install) {
                    assert!(body.contains("Herdr"), "names the app it shares the core with: {body}");
                }
            }
            let (_, body, foot) = setup_words_in(c, &needs[3], "", "", "https://releases");
            assert!(body.contains('2') && body.contains(&GENERATION.to_string()), "{body}");
            assert!(foot.contains("https://releases"));
            let (_, body, _) = setup_words_in(c, &needs[1], "", "", "");
            assert!(body.contains("0.1.9") && body.contains("0.1.11"), "{body}");
        }
    }

    #[test]
    fn a_core_that_comes_back_unable_to_serve_is_refused_and_said_once() {
        let t0 = Instant::now();
        for c in &catalogs() {
            let mut told = None;
            assert_eq!(serves_in(c, &hello(GENERATION, "9.9.9"), &mut told, t0), Ok(false), "nothing refused: no toast to close");
            assert!(told.is_none());

            let newer = serves_in(c, &hello(GENERATION + 1, "9.9.9"), &mut told, t0).unwrap_err().expect("the first time: a toast");
            let note = &newer["notification"];
            assert_eq!(newer["kind"], "toast");
            assert_eq!(note["level"], "error");
            assert!(note["timeout"].as_u64().unwrap() >= 3_600_000, "stays while Yuzora waits");
            assert_eq!(note["action"], json!({"name": "app.settings", "args": {"section": "updates"}}), "a click opens Updates");
            assert!(note["title"].as_str().unwrap().contains("Yuzora"));
            let body = note["body"].as_str().unwrap();
            assert!(body.contains("Herdr") && body.contains('2') && !body.contains('{'), "{body}");
            assert_eq!(note["timeout"].as_u64(), Some(TOAST_FOR.as_millis() as u64));
            let later = t0 + TOAST_FOR - Duration::from_secs(1);
            assert_eq!(serves_in(c, &hello(GENERATION + 1, "9.9.9"), &mut told, later), Err(None), "the same mismatch again: no new toast");
            let again = serves_in(c, &hello(GENERATION + 1, "9.9.9"), &mut told, t0 + TOAST_FOR).unwrap_err();
            assert_eq!(again.as_ref().map(|t| &t["notification"]["title"]), Some(&note["title"]), "its toast ran out: shown again while the mismatch lasts");

            let old = serves_in(c, &hello(GENERATION, "0.1.10"), &mut told, t0).unwrap_err().expect("another mismatch: a toast");
            assert!(old["notification"]["body"].as_str().unwrap().contains("0.1.10"));
            let missing = serves_in(c, &json!({"generation": GENERATION, "version": "9.9.9", "methods": USES_BUT_ONE}), &mut told, t0).unwrap_err().expect("a toast");
            assert!(missing["notification"]["body"].as_str().unwrap().contains("events.subscribe"));
            // The toast truncates title and body to one line each (mods/toast, 340 px):
            // what to do and why must fit, or the person never reads it.
            for toast in [&newer, &old, &missing] {
                for (part, max) in [("title", 42), ("body", 44)] {
                    let text = toast["notification"][part].as_str().unwrap();
                    assert!(width(text) <= max, "{part} too long for one line: {text}");
                }
            }
            assert_eq!(serves_in(c, &hello(GENERATION, "9.9.9"), &mut told, t0), Ok(true), "a core that serves is used again; close the toast");
            assert!(told.is_none());
        }
    }
}
