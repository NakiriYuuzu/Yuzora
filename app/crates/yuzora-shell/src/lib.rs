//! Yuzora L1（外殼工作區）: Workspace shell: the sidebar's workspace mode, mode bar, tab bar, status bar, the + new pane menu and mixed split tool panes.
//!
//! Owned by lane L1. Scripts reach it as `import { … } from "yuzora:shell"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! - [`workspace`]: the workspace sidebar's tree, attention list and scopes.
//! - [`badges`]: Spaces' badges (initial and kept color), for every header.
//! - [`git`]: a folder's branch and ahead/behind for the status bar.

pub mod badges;
pub mod git;
pub mod workspace;

use yuzora_core::host::{arg, error, opt_str, to_host};
use yuzora_core::{HostModule, HostValue};

/// The import specifier: `yuzora:shell`.
pub const NAME: &str = "yuzora:shell";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export type AgentState = "working" | "blocked" | "done" | "idle" | "unknown";
export interface SpaceBadge { initial: string; color: string }
export interface GitSummary { branch: string | null; upstream: string | null; ahead: number; behind: number; changed: number | null; conflicted: number | null }
export interface WorkspaceAgent {
  kind: "agent"; id: string; address: string; pane: string; space: string; space_name: string; tab: string;
  worktree: string; branch: string | null; name: string; brand: "claude" | "codex" | "pi" | "gemini" | null;
  icon: string | null; tint: boolean; status: AgentState; focused: boolean; pinned: boolean; machine: string | null;
}
export type WorkspaceRow =
  | { kind: "machine"; id: string; machine: string | null; open: boolean; led: "on" | "busy" | "off"; state: string | null; target: string | null; spaces: number; agents: number; blocked: number }
  | { kind: "space"; id: string; space: string; name: string; machine: string | null; initial: string; color: string; active: boolean; status: AgentState | null; branches: number; agents: number; blocked: number; done: number; folder: string; open: boolean }
  | { kind: "worktree"; id: string; space: string; label: string; branch: boolean; detail: string | null; folder: string | null; agents: number; blocked: number; open: boolean }
  | WorkspaceAgent;
export interface WorkspaceTree {
  /** The scope in effect: null = every machine, "" = this computer. */
  scope: string | null;
  scopes: { machine: string | null; led: "on" | "busy" | "off"; spaces: number; agents: number; blocked: number; active: boolean }[];
  rows: WorkspaceRow[];
  /** Agents in tree order, for setAgentOrder. */
  order: string[];
  attention: WorkspaceAgent[];
  /** The Agents tab: most urgent first. */
  agents: WorkspaceAgent[];
  flat_order: string[];
  counts: { spaces: number; agents: number; blocked: number; done: number; working: number };
  total: { spaces: number; agents: number; blocked: number; done: number };
}
/** The yuzora-shell crate's version (Yuzora's). */
export function version(): string;
/** This computer's name without its domain ("yuzu-mbp"), or null: the sidebar's "this computer" row. */
export function hostname(): string | null;
/** Spaces' badges: the first letter of each name and a color it keeps. Colors come from tokens().space_badges; pass
 *  shared("yuzora.space.colors") as kept. A Space not in it gets the least used color, and changed is true: write
 *  kept back with setShared("yuzora.space.colors", kept) so every mod and restart keeps it. Writes nothing itself. */
export function spaceBadges(spaces: { id: string; name: string }[], kept?: unknown): { badges: Record<string, SpaceBadge>; kept: Record<string, string>; changed: boolean };
/** The workspace sidebar's model from Herdr's live lists: spaces(), panes(), agents(), machines(), and hc machine ls --json's machines as hosts.
 *  folded: row ids. colors: shared("yuzora.space.colors"), as spaceBadges takes it. */
export function workspace(input: { spaces: unknown[]; panes: unknown[]; agents: unknown[]; machines?: unknown[]; hosts?: unknown[]; scope?: string | null; folded?: string[]; colors?: unknown }): WorkspaceTree;
/** An agent's brand from what it reported (app) or its program key: "claude", "codex", "pi", "gemini", or null. */
export function agentBrand(app: string | null, key: string | null): string | null;
/** A folder's branch and ahead/behind, here or on a machine (10 s a step); null outside a repo. Reads refs, config and
 *  commits only, so nothing the repo's own config names (filters, hooks, ssh) runs. changed and conflicted need
 *  git status: null until the folder is trusted. */
export function gitStatus(cwd: string, options?: { on?: string | null }): Promise<GitSummary | null>;
"#;

/// A host name without its domain; None when there's nothing left.
fn short_host(name: &str) -> Option<String> {
    Some(name.trim().split('.').next().unwrap_or_default().to_owned()).filter(|s| !s.is_empty())
}

/// A Space as `spaceBadges` takes it.
#[derive(serde::Deserialize)]
struct Named {
    id: String,
    #[serde(default)]
    name: String,
}

/// The `yuzora:shell` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .function("hostname", |_| Ok(short_host(&gethostname::gethostname().to_string_lossy()).map_or(HostValue::Null, HostValue::from)))
        .function("spaceBadges", |a| {
            let spaces: Vec<Named> = serde_json::from_value(arg(a, 0)).map_err(|e| error(format!("spaceBadges: {e}")))?;
            let spaces: Vec<(String, String)> = spaces.into_iter().map(|s| (s.id, s.name)).collect();
            let (badges, kept, changed) = badges::badges(&spaces, &arg(a, 1), &badges::palette());
            Ok(to_host(&serde_json::json!({ "badges": badges, "kept": kept, "changed": changed })))
        })
        .function("workspace", |a| {
            let input: workspace::Input = serde_json::from_value(arg(a, 0)).map_err(|e| error(format!("workspace: {e}")))?;
            Ok(to_host(&workspace::build(&input, &badges::palette())))
        })
        .function("agentBrand", |a| {
            let app = a.get(0).and_then(HostValue::as_str);
            let key = a.get(1).and_then(HostValue::as_str).unwrap_or_default();
            Ok(workspace::brand(app, key).map_or(HostValue::Null, HostValue::from))
        })
        .async_function("gitStatus", |a| {
            let cwd = a.string(0)?.to_owned();
            let on = opt_str(a, 1, "on");
            let (tx, rx) = async_channel::bounded(1);
            std::thread::Builder::new()
                .name("yuzora-shell-git".into())
                .spawn(move || {
                    let _ = tx.send_blocking(git::status(&cwd, on));
                })
                .map_err(error)?;
            Ok(async move {
                let found = rx.recv().await.map_err(|_| error("the git status was lost"))?.map_err(error)?;
                Ok(found.map_or(HostValue::Null, |s| to_host(&s.to_json())))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yuzora_core::HostArguments;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:shell");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
        assert!(m.is_async("gitStatus"));
        assert_eq!(short_host("yuzu-mbp.local\n"), Some("yuzu-mbp".into()));
        assert_eq!(short_host(".local"), None);
        assert!(matches!(m.call("hostname", &HostArguments::new([])).unwrap(), HostValue::Str(_) | HostValue::Null));
    }

    #[test]
    fn brands_come_back_as_strings_or_null() {
        let m = host_module();
        let call = |app: HostValue, key: &str| m.call("agentBrand", &HostArguments::new([app, HostValue::Str(key.into())])).unwrap();
        assert_eq!(call(HostValue::Str("claude-code".into()), "agent"), HostValue::Str("claude".into()));
        assert_eq!(call(HostValue::Null, "nvim"), HostValue::Null);
    }

    #[test]
    fn the_workspace_function_takes_herdrs_lists() {
        let m = host_module();
        let input = to_host(&json!({
            "spaces": [{ "id": "s1", "name": "yuzora" }], "panes": [], "agents": [],
            "machines": [{ "name": null, "layout": "ready" }],
            "hosts": [{ "name": "lab", "state": "connected", "via": true }],
            "colors": { "s1": "#123456" }
        }));
        let tree = yuzora_core::host::from_host(&m.call("workspace", &HostArguments::new([input])).unwrap());
        assert_eq!(tree["rows"][0]["kind"], "machine");
        assert_eq!(tree["rows"][1]["color"], "#123456");
        assert!(m.call("workspace", &HostArguments::new([to_host(&json!({ "spaces": 3 }))])).is_err());
    }

    #[test]
    fn space_badges_come_back_with_what_to_keep() {
        let m = host_module();
        let call = |spaces: serde_json::Value, kept: serde_json::Value| {
            yuzora_core::host::from_host(&m.call("spaceBadges", &HostArguments::new([to_host(&spaces), to_host(&kept)])).unwrap())
        };
        let first = call(json!([{ "id": "s1", "name": "yuzora" }, { "id": "s2", "name": "~" }]), json!(null));
        assert_eq!(first["badges"]["s1"]["initial"], "Y");
        assert_eq!(first["badges"]["s2"]["initial"], "~");
        assert_eq!(first["changed"], true);
        let again = call(json!([{ "id": "s1", "name": "yuzora" }, { "id": "s2", "name": "~" }]), first["kept"].clone());
        assert_eq!(again["changed"], false);
        assert_eq!(again["badges"], first["badges"]);
        assert!(m.call("spaceBadges", &HostArguments::new([HostValue::Str("x".into())])).is_err());
    }
}
