//! Starting an agent from a template, into a space and tab of this window.
//!
//! `hc run --json --bg --no-parent [--on <machine>] [--cwd <dir>] [--name <n>]
//! --label herdr.space=<id> --label herdr.tab=<id> -- <template> <task>`.
//!
//! The app shows any live terminal labeled `herdr.space`/`herdr.tab` in that
//! space and tab (its adoption: `reconcile` in the app's window), whatever
//! started it, so the agent lands where it was asked for. Ids are the app's
//! (`spaces()`); a space on another machine shows them as `<machine>/<id>`,
//! and the labels on that machine leave the prefix off. A tab id the app
//! doesn't know becomes a new tab in that space. The core document's name
//! (`yuzora`) plays no part: labels carry ids, not documents.
//!
//! An unknown template name would run as a program (`hc run` takes either),
//! so the caller checks the name against that machine's templates first.

use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub template: String,
    pub task: String,
    /// The space's machine (`None`: this computer).
    pub machine: Option<String>,
    /// The app's space id (`spaces()[i].id`, maybe `<machine>/…`).
    pub space: String,
    /// The app's tab id; `None` opens a new tab in the space.
    pub tab: Option<String>,
    pub cwd: Option<String>,
    /// A terminal name (`hc` names: 1–32 of a-z 0-9 - _).
    pub name: Option<String>,
}

/// An id as the labels on its machine write it.
pub fn local_id<'a>(id: &'a str, machine: Option<&str>) -> &'a str {
    machine.and_then(|m| id.strip_prefix(m).and_then(|s| s.strip_prefix('/'))).unwrap_or(id)
}

/// A fresh tab id for "a new tab": unique enough for one space.
pub fn new_tab_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("tpl{:x}", nanos & 0xffff_ffff_ffff)
}

pub fn valid_terminal_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// What's wrong with a launch before it runs (a code for the UI's strings).
pub fn check(l: &Launch) -> Option<&'static str> {
    if !crate::templates::valid_name(&l.template) {
        return Some("template_invalid");
    }
    if l.space.trim().is_empty() {
        return Some("space_missing");
    }
    if l.name.as_deref().is_some_and(|n| !valid_terminal_name(n)) {
        return Some("terminal_name_invalid");
    }
    None
}

/// `hc` arguments for a launch (the tab id is already chosen).
pub fn args(l: &Launch, tab: &str) -> Vec<String> {
    let machine = l.machine.as_deref().filter(|m| !m.is_empty());
    let mut a: Vec<String> = ["run", "--json", "--bg", "--no-parent"].map(String::from).into();
    if let Some(m) = machine {
        a.extend(["--on".into(), m.to_owned()]);
    }
    if let Some(cwd) = l.cwd.as_deref().filter(|c| !c.is_empty()) {
        a.extend(["--cwd".into(), cwd.to_owned()]);
    }
    if let Some(n) = l.name.as_deref().filter(|n| !n.is_empty()) {
        a.extend(["--name".into(), n.to_owned()]);
    }
    a.extend(["--label".into(), format!("herdr.space={}", local_id(&l.space, machine))]);
    a.extend(["--label".into(), format!("herdr.tab={}", local_id(tab, machine))]);
    a.push("--".into());
    a.push(l.template.clone());
    if !l.task.trim().is_empty() {
        a.push(l.task.clone());
    }
    a
}

/// `hc run --json`'s answer: the terminal, whether the task went in, or hc's error.
pub fn parse_result(code: i32, stdout: &str, stderr: &str) -> Value {
    let read = |s: &str| serde_json::from_str::<Value>(s.trim()).ok();
    if let Some(e) = read(stderr).or_else(|| read(stdout)).and_then(|v| v.get("error").cloned()) {
        return json!({ "ok": false, "kind": e.get("kind"), "message": e.get("message"), "hint": e.get("hint") });
    }
    match read(stdout) {
        Some(v) if code == 0 => json!({
            "ok": true,
            "address": v.get("address"),
            "name": v.get("name"),
            "sent": v.pointer("/first_message/sent").and_then(Value::as_bool).unwrap_or(false),
            "reason": v.pointer("/first_message/reason"),
        }),
        _ => json!({ "ok": false, "kind": "failed", "message": format!("{} {}", stdout.trim(), stderr.trim()).trim() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_the_agent_for_its_space_and_tab() {
        let l = Launch { template: "reviewer".into(), task: "--review src/x.rs".into(), space: "s1".into(), tab: Some("t2".into()), cwd: Some("/repo".into()), ..Launch::default() };
        assert_eq!(check(&l), None);
        assert_eq!(
            args(&l, "t2"),
            ["run", "--json", "--bg", "--no-parent", "--cwd", "/repo", "--label", "herdr.space=s1", "--label", "herdr.tab=t2", "--", "reviewer", "--review src/x.rs"]
        );
    }

    #[test]
    fn another_machines_ids_lose_their_prefix() {
        let l = Launch { template: "rev".into(), machine: Some("lab".into()), space: "lab/s9".into(), name: Some("rev-1".into()), ..Launch::default() };
        assert_eq!(
            args(&l, "lab/t4"),
            ["run", "--json", "--bg", "--no-parent", "--on", "lab", "--name", "rev-1", "--label", "herdr.space=s9", "--label", "herdr.tab=t4", "--", "rev"]
        );
        assert_eq!(local_id("other/s9", Some("lab")), "other/s9");
        assert_eq!(local_id("lab/s9", None), "lab/s9");
    }

    #[test]
    fn refuses_what_hc_would_misread() {
        let ok = Launch { template: "rev".into(), space: "s1".into(), ..Launch::default() };
        assert_eq!(check(&Launch { template: "../x".into(), ..ok.clone() }), Some("template_invalid"));
        assert_eq!(check(&Launch { space: " ".into(), ..ok.clone() }), Some("space_missing"));
        assert_eq!(check(&Launch { name: Some("Bad Name".into()), ..ok.clone() }), Some("terminal_name_invalid"));
        assert!(new_tab_id().starts_with("tpl"));
    }

    #[test]
    fn reads_hc_runs_answer() {
        let ok = parse_result(0, r#"{"address":"t1","first_message":{"sent":true,"turn":0},"name":"rv1","state":"live"}"#, "");
        assert_eq!(ok, json!({ "ok": true, "address": "t1", "name": "rv1", "sent": true, "reason": null }));
        let unsent = parse_result(0, r#"{"address":"t2","first_message":{"reason":"unknown key","sent":false}}"#, "");
        assert_eq!((unsent["sent"].as_bool(), unsent["reason"].as_str()), (Some(false), Some("unknown key")));
        let err = parse_result(1, "", r#"{"error":{"hint":"see hc templates","kind":"invalid_template","message":"no command"}}"#);
        assert_eq!(err["kind"], "invalid_template");
        assert_eq!(parse_result(1, "", "boom")["ok"], false);
    }
}
