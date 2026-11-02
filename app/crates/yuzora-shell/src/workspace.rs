//! The workspace sidebar's model: machines → Spaces → worktrees → agents, the
//! "needs attention" list, the scope chips and the flat Agents list.
//!
//! Pure: `yuzora-sidebar-workspace` passes Herdr's live lists in (`spaces()`,
//! `panes()`, `agents()`, `machines()`, `hc machine ls --json`) and draws what
//! comes back. Words people read (labels, "2 branches") stay in the mod's i18n.

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    pub spaces: Vec<Space>,
    pub panes: Vec<Pane>,
    pub agents: Vec<Agent>,
    /// Herdr's `machines()`: each machine's layout document (`name` null: this computer).
    pub machines: Vec<Machine>,
    /// `hc machine ls --json`'s machines: connection state and target.
    pub hosts: Vec<Host>,
    /// The scope chip: absent or null = every machine, "" = this computer, else a machine.
    pub scope: Option<String>,
    /// Folded rows' ids, as the rows give them (`m:…`, `s:…`, `w:…`).
    pub folded: Vec<String>,
    /// The Space colors given out so far (`shared("yuzora.space.colors")`).
    pub colors: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub machine: Option<String>,
    pub folder: String,
    pub branch: Option<String>,
    pub active: bool,
    pub status: Option<String>,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Tab {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Pane {
    pub pane: String,
    pub terminal: Option<String>,
    pub tab: String,
    pub cwd: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub address: String,
    pub pane: String,
    pub space: String,
    pub space_name: String,
    pub tab: String,
    pub status: Option<String>,
    pub app: Option<String>,
    pub program: Program,
    pub title: String,
    pub seq: f64,
    pub focused: bool,
    pub pinned: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Program {
    pub key: String,
    pub name: String,
    pub icon: Option<String>,
    pub tint: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub name: Option<String>,
    pub layout: Option<String>,
}

/// One of `hc machine ls --json`'s machines. Read leniently: a field of
/// another type than expected (`via` is a flag there) is left out rather than
/// failing the whole sidebar.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Host {
    #[serde(deserialize_with = "lenient")]
    pub name: String,
    #[serde(deserialize_with = "lenient")]
    pub state: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub target: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub temporary: bool,
}

fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).unwrap_or_default())
}

/// An agent's brand for its badge color (`tokens().agents`), from what it
/// reported (`app`: "claude-code") or its program table key ("claude").
pub fn brand(app: Option<&str>, key: &str) -> Option<&'static str> {
    [app.unwrap_or_default(), key].into_iter().find_map(|s| {
        let s = s.to_ascii_lowercase();
        if s.contains("claude") {
            Some("claude")
        } else if s.contains("codex") {
            Some("codex")
        } else if s.contains("gemini") {
            Some("gemini")
        } else if s == "pi" || s.starts_with("pi-") {
            Some("pi")
        } else {
            None
        }
    })
}

/// The five states the design draws; anything else (or nothing) is unknown.
pub fn status_word(s: Option<&str>) -> &'static str {
    match s {
        Some("working") => "working",
        Some("blocked") => "blocked",
        Some("done") => "done",
        Some("idle") => "idle",
        _ => "unknown",
    }
}

fn rank(status: &str) -> u8 {
    match status {
        "blocked" => 4,
        "done" => 3,
        "working" => 2,
        "idle" => 1,
        _ => 0,
    }
}

/// A machine's LED: `on` (connected), `busy` (reading its layout, connecting) or `off`.
fn led(local: bool, host: Option<&Host>, layout: Option<&str>) -> &'static str {
    if layout == Some("loading") {
        return "busy";
    }
    if local {
        return "on";
    }
    match host.and_then(|h| h.state.as_deref()) {
        Some("connecting" | "starting") => "busy",
        Some(state) if state != "connected" => "off",
        state => match layout {
            Some("ready") => "on",
            Some("unavailable") => "off",
            _ if state == Some("connected") => "on",
            _ => "off",
        },
    }
}

fn base(path: &str) -> &str {
    let p = path.trim_end_matches(['/', '\\']);
    p.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(p)
}

fn agent_name(a: &Agent) -> String {
    [a.program.name.as_str(), a.title.as_str(), a.address.as_str()]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default()
        .to_owned()
}

struct Ctx<'a> {
    panes: HashMap<&'a str, &'a Pane>,
    folded: HashSet<&'a str>,
}

impl<'a> Ctx<'a> {
    /// The worktree an agent works in: its pane's branch, else its folder's name.
    fn worktree(&self, a: &Agent) -> (String, bool, Option<String>) {
        let pane = self.panes.get(a.pane.as_str());
        let cwd = pane.and_then(|p| p.cwd.clone());
        match pane.and_then(|p| p.branch.clone()).filter(|b| !b.is_empty()) {
            Some(b) => (b, true, cwd),
            None => (cwd.as_deref().map(base).unwrap_or_default().to_owned(), false, cwd),
        }
    }

    fn agent(&self, a: &Agent) -> Value {
        let (worktree, is_branch, _) = self.worktree(a);
        let machine = a.address.split_once('/').map(|(m, _)| m);
        json!({
            "kind": "agent",
            "id": format!("a:{}", a.address),
            "address": a.address,
            "pane": a.pane,
            "space": a.space,
            "space_name": a.space_name,
            "tab": a.tab,
            "worktree": worktree,
            "branch": if is_branch { Some(worktree.as_str()) } else { None },
            "name": agent_name(a),
            "brand": brand(a.app.as_deref(), &a.program.key),
            "icon": a.program.icon,
            "tint": a.program.tint,
            "status": status_word(a.status.as_deref()),
            "focused": a.focused,
            "pinned": a.pinned,
            "machine": machine,
        })
    }

    fn open(&self, id: &str) -> bool {
        !self.folded.contains(id)
    }
}

/// The whole model for one render; Space colors from `palette`
/// (`tokens().space_badges`) and what `input.colors` kept.
pub fn build(input: &Input, palette: &[String]) -> Value {
    let named: Vec<(String, String)> = input.spaces.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
    let (badges, _, _) = crate::badges::badges(&named, &input.colors, palette);
    let cx = Ctx {
        panes: input.panes.iter().map(|p| (p.pane.as_str(), p)).collect(),
        folded: input.folded.iter().map(String::as_str).collect(),
    };

    // Machines: this computer, then the others by name (Herdr's space order).
    let mut remote: Vec<String> = input
        .machines
        .iter()
        .filter_map(|m| m.name.clone())
        .chain(input.hosts.iter().filter(|h| !h.temporary).map(|h| h.name.clone()))
        .chain(input.spaces.iter().filter_map(|s| s.machine.clone()))
        .filter(|n| !n.is_empty())
        .collect();
    remote.sort();
    remote.dedup();
    let machines: Vec<Option<String>> = std::iter::once(None).chain(remote.into_iter().map(Some)).collect();

    let host = |m: &Option<String>| m.as_ref().and_then(|n| input.hosts.iter().find(|h| &h.name == n));
    let layout = |m: &Option<String>| input.machines.iter().find(|x| x.name.as_ref().filter(|n| !n.is_empty()) == m.as_ref()).and_then(|x| x.layout.as_deref());

    // A scope naming a machine that's gone or offline shows everything (until it's back).
    let scope: Option<Option<String>> = match &input.scope {
        None => None,
        Some(s) if s.is_empty() => Some(None),
        Some(s) => machines
            .iter()
            .find(|m| m.as_deref() == Some(s))
            .filter(|m| led(false, host(m), layout(m)) != "off")
            .cloned(),
    };
    let in_scope = |m: &Option<String>| scope.as_ref().is_none_or(|s| s == m);
    let space_machine: HashMap<&str, Option<String>> = input.spaces.iter().map(|s| (s.id.as_str(), s.machine.clone().filter(|m| !m.is_empty()))).collect();
    let agent_in_scope = |a: &Agent| space_machine.get(a.space.as_str()).is_some_and(in_scope);
    let scoped_agents: Vec<&Agent> = input.agents.iter().filter(|a| agent_in_scope(a)).collect();

    let count = |list: &[&Agent], word: &str| list.iter().filter(|a| status_word(a.status.as_deref()) == word).count();
    let per_machine = |m: &Option<String>| {
        let spaces = input.spaces.iter().filter(|s| &s.machine.clone().filter(|x| !x.is_empty()) == m).count();
        let agents: Vec<&Agent> = input.agents.iter().filter(|a| space_machine.get(a.space.as_str()) == Some(m)).collect();
        (spaces, agents.len(), count(&agents, "blocked"))
    };
    let scopes: Vec<Value> = machines
        .iter()
        .map(|m| {
            let (spaces, agents, blocked) = per_machine(m);
            json!({
                "machine": m,
                "led": led(m.is_none(), host(m), layout(m)),
                "spaces": spaces, "agents": agents, "blocked": blocked,
                "active": scope.as_ref() == Some(m),
            })
        })
        .collect();

    // The tree, and the agents in its order (agent.next / agent.prev follow it).
    let mut rows = Vec::new();
    let mut order = Vec::new();
    for m in machines.iter().filter(|m| in_scope(m)) {
        let (spaces, agents, blocked) = per_machine(m);
        let id = format!("m:{}", m.as_deref().unwrap_or_default());
        let machine_open = cx.open(&id);
        let h = host(m);
        rows.push(json!({
            "kind": "machine", "id": id, "machine": m, "open": machine_open,
            "led": led(m.is_none(), h, layout(m)),
            "state": h.and_then(|h| h.state.clone()),
            "target": h.and_then(|h| h.target.clone()),
            "spaces": spaces, "agents": agents, "blocked": blocked,
        }));
        for s in input.spaces.iter().filter(|s| &s.machine.clone().filter(|x| !x.is_empty()) == m) {
            let tabs: HashSet<&str> = s.tabs.iter().map(|t| t.id.as_str()).collect();
            let mut branches: Vec<&str> = input.panes.iter().filter(|p| tabs.contains(p.tab.as_str())).filter_map(|p| p.branch.as_deref()).collect();
            branches.extend(s.branch.as_deref());
            branches.retain(|b| !b.is_empty());
            branches.sort_unstable();
            branches.dedup();
            let mine: Vec<&Agent> = input.agents.iter().filter(|a| a.space == s.id).collect();
            let id = format!("s:{}", s.id);
            let space_open = cx.open(&id);
            if machine_open {
                let badge = &badges[&s.id];
                rows.push(json!({
                    "kind": "space", "id": id, "space": s.id, "name": s.name, "machine": m,
                    "initial": badge["initial"], "color": badge["color"], "active": s.active,
                    "status": s.status.as_deref().map(|w| status_word(Some(w))),
                    "branches": branches.len(), "agents": mine.len(),
                    "blocked": count(&mine, "blocked"), "done": count(&mine, "done"),
                    "folder": s.folder, "open": space_open,
                }));
            }
            let open = machine_open && space_open;
            // Worktrees (a branch or folder name in one folder) in the order their first agent
            // comes. The same branch in two folders is two rows, each saying which folder.
            let mut groups: Vec<(String, bool, Option<String>, Vec<&Agent>)> = Vec::new();
            for a in &mine {
                let (key, is_branch, cwd) = cx.worktree(a);
                match groups.iter_mut().find(|g| g.0 == key && g.2 == cwd) {
                    Some(g) => g.3.push(a),
                    None => groups.push((key, is_branch, cwd, vec![a])),
                }
            }
            let shared: HashSet<&str> = groups.iter().enumerate().filter(|(i, g)| groups[..*i].iter().any(|o| o.0 == g.0)).map(|(_, g)| g.0.as_str()).collect();
            for (key, is_branch, cwd, list) in &groups {
                order.extend(list.iter().map(|a| a.address.clone()));
                if !open {
                    continue;
                }
                let id = format!("w:{}\u{1f}{key}\u{1f}{}", s.id, cwd.as_deref().unwrap_or_default());
                let wt_open = cx.open(&id);
                // Told apart by the folder's name (the whole path when that is the label already).
                let detail = shared.contains(key.as_str()).then(|| cwd.as_deref().map(|c| if *is_branch { base(c) } else { c })).flatten();
                rows.push(json!({
                    "kind": "worktree", "id": id, "space": s.id, "label": key, "branch": is_branch, "detail": detail,
                    "folder": cwd, "agents": list.len(), "blocked": count(list, "blocked"), "open": wt_open,
                }));
                if wt_open {
                    rows.extend(list.iter().map(|a| cx.agent(a)));
                }
            }
        }
    }

    // Who needs the person: blocked first, then finished-unseen, the latest first.
    let mut attention: Vec<&Agent> = scoped_agents.iter().copied().filter(|a| matches!(status_word(a.status.as_deref()), "blocked" | "done")).collect();
    attention.sort_by(|a, b| {
        rank(status_word(b.status.as_deref()))
            .cmp(&rank(status_word(a.status.as_deref())))
            .then(b.seq.partial_cmp(&a.seq).unwrap_or(std::cmp::Ordering::Equal))
    });

    // The Agents tab: one flat list, most urgent first, otherwise Herdr's order.
    let mut flat = scoped_agents.clone();
    flat.sort_by_key(|a| std::cmp::Reverse(rank(status_word(a.status.as_deref()))));

    let all: Vec<&Agent> = input.agents.iter().collect();
    json!({
        "scope": scope.clone().map(|m| m.unwrap_or_default()),
        "scopes": scopes,
        "rows": rows,
        "order": order,
        "attention": attention.iter().map(|a| cx.agent(a)).collect::<Vec<_>>(),
        "agents": flat.iter().map(|a| cx.agent(a)).collect::<Vec<_>>(),
        "flat_order": flat.iter().map(|a| a.address.clone()).collect::<Vec<_>>(),
        "counts": {
            "spaces": input.spaces.iter().filter(|s| in_scope(&s.machine.clone().filter(|m| !m.is_empty()))).count(),
            "agents": scoped_agents.len(),
            "blocked": count(&scoped_agents, "blocked"),
            "done": count(&scoped_agents, "done"),
            "working": count(&scoped_agents, "working"),
        },
        "total": {
            "spaces": input.spaces.len(),
            "agents": all.len(),
            "blocked": count(&all, "blocked"),
            "done": count(&all, "done"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(v: Value) -> Input {
        serde_json::from_value(v).unwrap()
    }

    fn palette() -> Vec<String> {
        ["#a", "#b", "#c"].map(String::from).to_vec()
    }

    fn world() -> Value {
        json!({
            "spaces": [
                { "id": "s1", "name": "yuzora", "machine": null, "active": true, "status": "blocked", "folder": "/r/yuzora", "tabs": [{ "id": "t1" }, { "id": "t2" }] },
                { "id": "s2", "name": "herdr-bridge", "machine": null, "status": "blocked", "tabs": [{ "id": "t3" }] },
                { "id": "lab/s1", "name": "infra", "machine": "lab", "tabs": [{ "id": "lab/t1" }] }
            ],
            "panes": [
                { "pane": "p1", "terminal": "t1a", "tab": "t1", "cwd": "/r/yuzora", "branch": "feat/dock" },
                { "pane": "p2", "terminal": "t1b", "tab": "t1", "cwd": "/r/yuzora", "branch": "feat/dock" },
                { "pane": "p3", "terminal": "t2a", "tab": "t2", "cwd": "/w/sftp", "branch": "fix/sftp" },
                { "pane": "p4", "terminal": "t3a", "tab": "t3", "cwd": "/r/bridge", "branch": "main" },
                { "pane": "p5", "terminal": null, "tab": "t3" },
                { "pane": "lab/p1", "terminal": "lab/t9", "tab": "lab/t1", "cwd": "/srv/infra", "branch": null }
            ],
            "agents": [
                { "address": "t1a", "pane": "p1", "space": "s1", "space_name": "yuzora", "tab": "t1", "status": "working", "app": "claude-code", "program": { "key": "claude", "name": "Claude Code" }, "seq": 5, "focused": true },
                { "address": "t1b", "pane": "p2", "space": "s1", "space_name": "yuzora", "tab": "t1", "status": "done", "app": "pi", "program": { "key": "pi", "name": "Pi" }, "seq": 9 },
                { "address": "t2a", "pane": "p3", "space": "s1", "space_name": "yuzora", "tab": "t2", "status": "blocked", "program": { "key": "codex", "name": "Codex" }, "seq": 3 },
                { "address": "t3a", "pane": "p4", "space": "s2", "space_name": "herdr-bridge", "tab": "t3", "status": "blocked", "program": { "key": "claude", "name": "Claude Code" }, "seq": 7 },
                { "address": "lab/t9", "pane": "lab/p1", "space": "lab/s1", "space_name": "infra", "tab": "lab/t1", "status": "idle", "program": { "key": "shell", "name": "" }, "title": "bash", "seq": 1 }
            ],
            "machines": [{ "name": null, "layout": "ready" }, { "name": "lab", "layout": "ready" }],
            "hosts": [
                { "name": "lab", "state": "connected", "target": "yuzu@lab", "via": false },
                { "name": "box", "state": "unreachable", "via": true },
                { "name": "tmp", "temporary": true }
            ],
            "colors": { "s2": "#c" }
        })
    }

    fn kinds(tree: &Value) -> Vec<String> {
        tree["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let k = r["kind"].as_str().unwrap();
                let what = match k {
                    "machine" => r["machine"].as_str().unwrap_or("local").to_owned(),
                    "space" => r["name"].as_str().unwrap().to_owned(),
                    "worktree" => r["label"].as_str().unwrap().to_owned(),
                    _ => r["address"].as_str().unwrap().to_owned(),
                };
                format!("{k}:{what}")
            })
            .collect()
    }

    #[test]
    fn the_tree_nests_machines_spaces_worktrees_and_agents() {
        let tree = build(&input(world()), &palette());
        assert_eq!(
            kinds(&tree),
            [
                "machine:local", "space:yuzora", "worktree:feat/dock", "agent:t1a", "agent:t1b", "worktree:fix/sftp", "agent:t2a",
                "space:herdr-bridge", "worktree:main", "agent:t3a",
                "machine:box", "machine:lab", "space:infra", "worktree:infra", "agent:lab/t9",
            ]
        );
        let rows = tree["rows"].as_array().unwrap();
        let yuzora = &rows[1];
        assert_eq!((yuzora["branches"].as_u64(), yuzora["agents"].as_u64(), yuzora["blocked"].as_u64()), (Some(2), Some(3), Some(1)));
        // s2 kept #c; s1 and lab/s1 get the least used ones, in order.
        assert_eq!((yuzora["initial"].as_str(), yuzora["color"].as_str()), (Some("Y"), Some("#a")));
        assert_eq!(rows[7]["color"], "#c");
        assert_eq!(rows[12]["color"], "#b");
        assert_eq!(rows[5]["blocked"], 1);
        assert_eq!(rows[5]["branch"], true);
        // No branch there: the worktree is named after its folder.
        assert_eq!(rows[13]["branch"], false);
        assert_eq!(rows[3]["brand"], "claude");
        assert_eq!(rows[4]["brand"], "pi");
        assert_eq!(rows[14]["name"], "bash");
        assert_eq!(rows[14]["machine"], "lab");
        assert_eq!(tree["order"], json!(["t1a", "t1b", "t2a", "t3a", "lab/t9"]));
    }

    #[test]
    fn machines_light_up_by_connection_and_layout() {
        let tree = build(&input(world()), &palette());
        let leds: Vec<(Value, Value)> = tree["scopes"].as_array().unwrap().iter().map(|s| (s["machine"].clone(), s["led"].clone())).collect();
        assert_eq!(leds, [(json!(null), json!("on")), (json!("box"), json!("off")), (json!("lab"), json!("on"))]);
        assert_eq!(led(false, None, Some("loading")), "busy");
        assert_eq!(led(false, None, None), "off");
        assert_eq!(led(true, None, None), "on");
        assert_eq!(tree["scopes"][0]["spaces"], 2);
        assert_eq!(tree["scopes"][0]["blocked"], 2);
        assert!(tree["scope"].is_null());
    }

    #[test]
    fn attention_lists_blocked_then_done_latest_first() {
        let tree = build(&input(world()), &palette());
        let who: Vec<&str> = tree["attention"].as_array().unwrap().iter().map(|a| a["address"].as_str().unwrap()).collect();
        assert_eq!(who, ["t3a", "t2a", "t1b"]);
        assert_eq!(tree["counts"]["blocked"], 2);
        assert_eq!(tree["counts"]["done"], 1);
        assert_eq!(tree["total"]["agents"], 5);
    }

    #[test]
    fn the_flat_list_puts_the_urgent_first_and_keeps_herdrs_order_otherwise() {
        let tree = build(&input(world()), &palette());
        assert_eq!(tree["flat_order"], json!(["t2a", "t3a", "t1b", "t1a", "lab/t9"]));
        assert_eq!(tree["agents"][0]["space_name"], "yuzora");
    }

    #[test]
    fn a_scope_shows_one_machine_and_a_gone_one_shows_all() {
        let mut w = world();
        w["scope"] = json!("lab");
        let tree = build(&input(w.clone()), &palette());
        assert_eq!(kinds(&tree), ["machine:lab", "space:infra", "worktree:infra", "agent:lab/t9"]);
        assert_eq!(tree["scope"], "lab");
        assert_eq!(tree["counts"]["agents"], 1);
        assert_eq!(tree["total"]["agents"], 5);
        assert!(tree["attention"].as_array().unwrap().is_empty());
        assert_eq!(tree["scopes"][2]["active"], true);

        w["scope"] = json!("");
        let tree = build(&input(w.clone()), &palette());
        assert_eq!(tree["scope"], "");
        assert_eq!(tree["counts"]["spaces"], 2);

        w["scope"] = json!("gone");
        let tree = build(&input(w.clone()), &palette());
        assert!(tree["scope"].is_null());
        assert_eq!(tree["counts"]["spaces"], 3);

        // Offline (box is unreachable, lab's layout can't be read): every machine until it's back.
        w["scope"] = json!("box");
        assert!(build(&input(w.clone()), &palette())["scope"].is_null());
        w["scope"] = json!("lab");
        w["machines"][1]["layout"] = json!("unavailable");
        w["hosts"][0]["state"] = json!("disconnected");
        assert!(build(&input(w.clone()), &palette())["scope"].is_null());
        // Still connecting: the scope holds.
        w["hosts"][0]["state"] = json!("connecting");
        assert_eq!(build(&input(w), &palette())["scope"], "lab");
    }

    #[test]
    fn the_same_branch_in_two_folders_is_two_worktrees() {
        let tree = build(
            &input(json!({
                "spaces": [{ "id": "s1", "name": "mono", "tabs": [{ "id": "t1" }] }],
                "panes": [
                    { "pane": "p1", "tab": "t1", "cwd": "/r/api", "branch": "main" },
                    { "pane": "p2", "tab": "t1", "cwd": "/r/web", "branch": "main" },
                    { "pane": "p3", "tab": "t1", "cwd": "/r/api", "branch": "main" },
                    { "pane": "p4", "tab": "t1", "cwd": "/r/docs", "branch": "feat" }
                ],
                "agents": [
                    { "address": "a1", "pane": "p1", "space": "s1" },
                    { "address": "a2", "pane": "p2", "space": "s1" },
                    { "address": "a3", "pane": "p3", "space": "s1" },
                    { "address": "a4", "pane": "p4", "space": "s1" }
                ]
            })),
            &palette(),
        );
        let wt: Vec<(Value, Value, Value)> = tree["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "worktree")
            .map(|r| (r["label"].clone(), r["detail"].clone(), r["folder"].clone()))
            .collect();
        assert_eq!(
            wt,
            [
                (json!("main"), json!("api"), json!("/r/api")),
                (json!("main"), json!("web"), json!("/r/web")),
                (json!("feat"), json!(null), json!("/r/docs")),
            ]
        );
        assert_eq!(tree["order"], json!(["a1", "a3", "a2", "a4"]));
    }

    #[test]
    fn folding_hides_children_but_keeps_the_agent_order() {
        let mut w = world();
        w["folded"] = json!(["s:s1", "w:s2\u{1f}main\u{1f}/r/bridge", "m:lab"]);
        let tree = build(&input(w), &palette());
        assert_eq!(kinds(&tree), ["machine:local", "space:yuzora", "space:herdr-bridge", "worktree:main", "machine:box", "machine:lab"]);
        assert_eq!(tree["rows"][1]["open"], false);
        assert_eq!(tree["rows"][3]["open"], false);
        assert_eq!(tree["order"], json!(["t1a", "t1b", "t2a", "t3a", "lab/t9"]));
    }

    #[test]
    fn brands_and_states_have_fallbacks() {
        assert_eq!(brand(Some("claude-code"), "agent"), Some("claude"));
        assert_eq!(brand(None, "codex"), Some("codex"));
        assert_eq!(brand(Some(""), "gemini"), Some("gemini"));
        assert_eq!(brand(None, "pi"), Some("pi"));
        assert_eq!(brand(None, "pip"), None);
        assert_eq!(brand(None, "nvim"), None);
        assert_eq!(status_word(Some("error")), "unknown");
        assert_eq!(status_word(None), "unknown");
        assert_eq!(base("/a/b/"), "b");
        assert_eq!(base("C:\\x\\y"), "y");
        assert_eq!(base("/"), "");
    }

    #[test]
    fn hosts_with_unexpected_fields_still_read() {
        let hosts: Vec<Host> = serde_json::from_value(json!([
            { "name": "lab", "state": "connected", "via": "ssh lab", "target": 4, "temporary": "yes", "extra": [1] },
            { "name": null, "state": { "x": 1 } }
        ]))
        .unwrap();
        assert_eq!((hosts[0].name.as_str(), hosts[0].state.as_deref(), hosts[0].target.as_deref(), hosts[0].temporary), ("lab", Some("connected"), None, false));
        assert_eq!((hosts[1].name.as_str(), hosts[1].state.as_deref()), ("", None));
    }

    #[test]
    fn an_empty_world_still_has_this_computer() {
        let tree = build(&Input::default(), &palette());
        assert_eq!(kinds(&tree), ["machine:local"]);
        assert_eq!(tree["scopes"].as_array().unwrap().len(), 1);
    }
}
