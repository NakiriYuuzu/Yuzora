//! Yuzora L1（外殼工作區）: Workspace shell: the sidebar's workspace mode, mode bar, tab bar, status bar, the + new pane menu and mixed split tool panes.
//!
//! Owned by lane L1. Scripts reach it as `import { … } from "yuzora:shell"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].
//!
//! - [`workspace`]: the workspace sidebar's tree, attention list and scopes.
//! - [`badges`]: a Space's badge (initial and kept color), for every header.
//! - [`git`]: a folder's branch and ahead/behind for the status bar.

pub mod badges;
pub mod git;
pub mod workspace;

use yuzora_core::host::{arg, error, opt_str, to_host};
use yuzora_core::{HostModule, HostObject, HostValue};

/// The import specifier: `yuzora:shell`.
pub const NAME: &str = "yuzora:shell";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export type AgentState = "working" | "blocked" | "done" | "idle" | "unknown";
export interface SpaceBadge { initial: string; color: string }
export interface GitSummary { branch: string | null; upstream: string | null; ahead: number; behind: number; changed: number; conflicted: number }
export interface WorkspaceAgent {
  kind: "agent"; id: string; address: string; pane: string; space: string; space_name: string; tab: string;
  worktree: string; branch: string | null; name: string; brand: "claude" | "codex" | "pi" | "gemini" | null;
  icon: string | null; tint: boolean; status: AgentState; focused: boolean; pinned: boolean; machine: string | null;
}
export type WorkspaceRow =
  | { kind: "machine"; id: string; machine: string | null; open: boolean; led: "on" | "busy" | "off"; state: string | null; target: string | null; via: string | null; spaces: number; agents: number; blocked: number }
  | { kind: "space"; id: string; space: string; name: string; machine: string | null; initial: string; color: string; active: boolean; status: AgentState | null; branches: number; agents: number; blocked: number; done: number; folder: string; open: boolean }
  | { kind: "worktree"; id: string; space: string; label: string; branch: boolean; folder: string | null; agents: number; blocked: number; open: boolean }
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
/** A Space's badge: the first letter of its name and a color it keeps (tokens().space_badges, kept in Yuzora's settings.json shell.space_colors). */
export function spaceBadge(id: string, name: string): SpaceBadge;
/** The workspace sidebar's model from Herdr's live lists: spaces(), panes(), agents(), machines(), and hc machine ls --json's machines as hosts. folded: row ids. */
export function workspace(input: { spaces: unknown[]; panes: unknown[]; agents: unknown[]; machines?: unknown[]; hosts?: unknown[]; scope?: string | null; folded?: string[] }): WorkspaceTree;
/** An agent's brand from what it reported (app) or its program key: "claude", "codex", "pi", "gemini", or null. */
export function agentBrand(app: string | null, key: string | null): string | null;
/** A folder's branch, ahead/behind and changed files, here or on a machine (10 s); null outside a repo. */
export function gitStatus(cwd: string, options?: { on?: string | null }): Promise<GitSummary | null>;
"#;

/// The `yuzora:shell` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
        .function("spaceBadge", |a| {
            let (initial, color) = badges::badge(a.string(0)?, a.get(1).and_then(HostValue::as_str).unwrap_or_default());
            Ok(HostObject::new().field("initial", initial).field("color", color).into())
        })
        .function("workspace", |a| {
            let input: workspace::Input = serde_json::from_value(arg(a, 0)).map_err(|e| error(format!("workspace: {e}")))?;
            Ok(to_host(&workspace::build(&input, &mut badges::badge)))
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
        // No spaces: no badge is asked for, so nothing is written to the app home.
        let input = to_host(&json!({ "spaces": [], "panes": [], "agents": [], "machines": [{ "name": null, "layout": "ready" }] }));
        let tree = yuzora_core::host::from_host(&m.call("workspace", &HostArguments::new([input])).unwrap());
        assert_eq!(tree["rows"][0]["kind"], "machine");
        assert!(m.call("workspace", &HostArguments::new([to_host(&json!({ "spaces": 3 }))])).is_err());
    }
}
