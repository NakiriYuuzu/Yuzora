import type { DbProfileLoadResult, DbDescriptorId } from "@/lib/types";
import { sampleQuery } from "./query";
/** In-memory transport for the public demo. Never imported by the desktop entry. */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { GitStatus, DbQueryRunRequest } from "@/lib/types";

export const ROOT = "/demo/evening-studio";
export const files: Record<string, string> = {
  "README.md":
    "# Evening Studio\n\nA small space for your next big idea.\n\n## Try the workbench\n\n- Open a file from the right sidebar\n- Edit code and switch tabs\n- Explore Git changes\n- Run a sample SQL query\n- Pick your favorite theme in Settings\n\nEverything here uses sample data.\n",
  "src/App.tsx":
    'import { useState } from "react"\nimport { BrandMark } from "./BrandMark"\n\nexport function EveningStudio() {\n  const [message, setMessage] = useState("Hello, evening sky.")\n\n  return (\n    <main className="evening-studio">\n      <BrandMark />\n      <h1>{message}</h1>\n      <p>A little space for your next big idea.</p>\n      <button onClick={() => setMessage("Let’s make something.")}>\n        Start creating\n      </button>\n    </main>\n  )\n}\n',
  "src/theme.css":
    ":root {\n  --sky: #2f6bff;\n  --sea: #14b8a6;\n  --paper: #f6f5ef;\n}\n\n.evening-studio {\n  background: var(--paper);\n  color: var(--sky);\n  padding: 48px;\n}\n",
  "package.json":
    '{\n  "name": "evening-studio",\n  "scripts": { "dev": "vite", "test": "vitest" },\n  "dependencies": { "react": "19.2.8" }\n}\n',
  "queries/agents.sql":
    "SELECT name, role, status\nFROM agents\nORDER BY name;\n",
};
export const status: GitStatus = {
  branch: "feature/evening-sky",
  headOid: "d08ca21",
  detached: false,
  upstream: "origin/feature/evening-sky",
  ahead: 1,
  behind: 0,
  staged: [],
  unstaged: [
    { path: "src/App.tsx", origPath: null, status: "M" },
    { path: "src/theme.css", origPath: null, status: "M" },
  ],
  untracked: ["queries/agents.sql"],
  conflicted: [],
  inProgress: null,
};
export const branches = {
  local: [
    {
      name: "feature/evening-sky",
      upstream: "origin/feature/evening-sky",
      ahead: 1,
      behind: 0,
      isCurrent: true,
      gone: false,
    },
    {
      name: "main",
      upstream: "origin/main",
      ahead: 0,
      behind: 0,
      isCurrent: false,
      gone: false,
    },
  ],
  remote: ["origin/main", "origin/feature/evening-sky"],
  tags: [],
};
export const environment = {
  status: "ready" as const,
  root: ROOT,
  version: "2.49.0",
};
export const sessions = [
  {
    name: "studio",
    default: true,
    running: true,
    sessionDir: "/demo/studio",
    socketPath: "/demo/studio.sock",
  },
];
export const snapshot = {
  protocol: 22,
  version: "0.9.0",
  snapshot: {
    version: "0.9.0",
    protocol: 22,
    focused_workspace_id: "studio",
    focused_tab_id: "build",
    focused_pane_id: "pane-build",
    workspaces: [
      {
        workspace_id: "studio",
        label: "Evening Studio",
        number: 0,
        focused: true,
        path: ROOT,
        active_tab_id: "build",
        tab_count: 2,
        pane_count: 2,
      },
      {
        workspace_id: "notes",
        label: "Little Ideas",
        number: 1,
        focused: false,
        path: "/demo/little-ideas",
        active_tab_id: "ideas",
        tab_count: 1,
        pane_count: 1,
      },
    ],
    tabs: [
      {
        tab_id: "build",
        workspace_id: "studio",
        label: "Codex · build",
        number: 0,
        focused: true,
        pane_count: 1,
        agent_status: "working",
      },
      {
        tab_id: "review",
        workspace_id: "studio",
        label: "Review",
        number: 1,
        focused: false,
        pane_count: 1,
        agent_status: "done",
      },
      {
        tab_id: "ideas",
        workspace_id: "notes",
        label: "Ideas",
        number: 0,
        focused: false,
        pane_count: 1,
        agent_status: "idle",
      },
    ],
    panes: ["build", "review", "ideas"].map((id) => ({
      pane_id: `pane-${id}`,
      terminal_id: `term-${id}`,
      tab_id: id,
      workspace_id: id === "ideas" ? "notes" : "studio",
      focused: id === "build",
      revision: 1,
      agent_status:
        id === "build" ? "working" : id === "review" ? "done" : "idle",
    })),
    agents: ["build", "review", "ideas"].map((id) => ({
      terminal_id: `term-${id}`,
      pane_id: `pane-${id}`,
      tab_id: id,
      workspace_id: id === "ideas" ? "notes" : "studio",
      name: id === "build" ? "Codex" : id === "review" ? "Claude" : "Pi",
      agent_status:
        id === "build" ? "working" : id === "review" ? "done" : "idle",
      revision: 1,
    })),
    layouts: [],
  },
};
const methods = [
  "snapshot",
  "ping",
  "tabCreate",
  "workspaceFocus",
  "workspaceCreate",
  "workspaceRename",
  "workspaceClose",
  "tabRename",
  "tabClose",
  "tabFocus",
  "tabMove",
  "paneFocus",
  "paneRename",
  "paneSplit",
  "paneZoom",
  "paneSwap",
  "paneClose",
  "layoutExport",
  "layoutSetSplitRatio",
  "agentGet",
  "agentRead",
  "eventsSubscribe",
  "worktreeList",
];
export const capabilities = {
  binaryPath: "/demo/herdr",
  binaryVersion: "0.9.0",
  binaryProtocol: 22,
  channel: "stable",
  binarySource: {
    configured: "default",
    resolved: "default",
    available: true,
    path: "/demo/herdr",
    restartRequired: false,
  },
  server: { running: true, version: "0.9.0", protocol: 22, compatible: true },
  api: {
    ...Object.fromEntries(methods.map((x) => [x, true])),
    methods: [
      "session.snapshot",
      "workspace.focus",
      "tab.focus",
      "layout.export",
    ],
    schemaProtocol: 22,
    schemaVersion: 1,
  },
  terminal: {
    observe: true,
    control: true,
    takeover: true,
    input: true,
    resize: true,
    scroll: true,
    release: true,
    create: true,
  },
  events: { status: "deferred" },
};
const terminals = new Map<
  string,
  {
    channel: Channel<unknown>;
    seq: number;
    input: string;
    cols: number;
    rows: number;
  }
>();
let nextTerminal = 0;
export const terminalIntro =
  "\x1b[38;2;47;107;255m  ✦ Evening Studio\x1b[0m\r\n  Yuzora × HERDR · interactive preview\r\n\r\n\x1b[2m  Sample agent session — no commands run on your device.\x1b[0m\r\n\r\n  › Build a little space for our next idea.\r\n\r\n  \x1b[32m✓\x1b[0m Created the workspace and theme tokens\r\n  \x1b[32m✓\x1b[0m Connected the geometric Yuzora identity\r\n  \x1b[32m✓\x1b[0m Added a friendly Space companion\r\n\r\n  src/App.tsx          \x1b[32m+12\x1b[0m  \x1b[31m−3\x1b[0m\r\n  src/theme.css        \x1b[32m+8\x1b[0m   \x1b[31m−2\x1b[0m\r\n\r\n  Try: help · ls · git status · bun test · clear\r\n\r\n\x1b[36m~/evening-studio\x1b[0m  \x1b[35mfeature/evening-sky\x1b[0m\r\n$ ";
function output(id: string, text: string, full = false) {
  const entry = terminals.get(id);
  if (entry)
    entry.channel.onmessage({
      type: "frame",
      sessionId: id,
      seq: entry.seq++,
      full,
      encoding: "ansi",
      width: entry.cols,
      height: entry.rows,
      bytesBase64: btoa(String.fromCharCode(...new TextEncoder().encode(text))),
    });
}
export const dbProfile = {
  id: "demo-db",
  configGeneration: 1,
  targetKey: "sqlite:/demo/studio.sqlite",
  name: "Studio · SQLite",
  kind: "sqlite" as const,
  path: "/demo/studio.sqlite",
  credentialState: "notRequired" as const,
};
export const dbTables = ["agents", "sessions", "workspaces"].map((name) => ({
  catalog: "main",
  schema: "main",
  name,
  kind: "table" as const,
}));

export function installDemoRuntime() {
  mockWindows("main");
  mockIPC(
    async (command, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      const path = String(args.path ?? "");
      switch (command) {
        case "open_workspace":
          return { path, canonicalPath: path, capabilityId: "demo-workspace" };
        case "workspace_canonical_path":
          return path;
        case "workspace_trust_check":
          return { state: "trusted", canonicalPath: path };
        case "list_dir": {
          const prefix = path.replace(ROOT, "").replace(/^\//, "");
          const names = new Map<string, boolean>();
          for (const key of Object.keys(files)) {
            if (prefix && !key.startsWith(prefix + "/")) continue;
            const rest = prefix ? key.slice(prefix.length + 1) : key;
            const name = rest.split("/")[0];
            names.set(name, rest.includes("/"));
          }
          return [...names]
            .map(([name, isDir]) => ({
              name,
              path: `${path}/${name}`,
              isDir,
              kind: isDir ? "directory" : "file",
            }))
            .sort(
              (a, b) =>
                Number(b.isDir) - Number(a.isDir) ||
                a.name.localeCompare(b.name),
            );
        }
        case "open_file": {
          const content =
            files[path.replace(ROOT + "/", "")] ?? files["README.md"];
          return {
            kind: "full",
            content,
            size: content.length,
            lineEnding: "lf",
          };
        }
        case "save_file":
          files[path.replace(ROOT + "/", "")] = String(args.content);
          return String(args.content).length;
        case "is_openable_file":
          return true;
        case "git_bootstrap":
          return { environment, status, branches };
        case "git_detect":
          return environment;
        case "git_status_cmd":
          return { ...status };
        case "git_branches":
          return branches;
        case "git_remote_probe":
          return "no";
        case "git_diff_content": {
          const content =
            files[path.replace(ROOT + "/", "")] ?? files["src/App.tsx"];
          return {
            original: {
              kind: "full",
              content: content
                .replace("Hello, evening sky.", "Hello, world.")
                .replace(
                  "A little space for your next big idea.",
                  "Welcome to our app.",
                ),
            },
            modified: { kind: "full", content },
          };
        }
        case "git_stage": {
          const paths = args.paths as string[];
          status.staged = [
            ...status.staged,
            ...status.unstaged.filter((x) => paths.includes(x.path)),
            ...status.untracked
              .filter((path) => paths.includes(path))
              .map((path) => ({ path, origPath: null, status: "A" })),
          ];
          status.untracked = status.untracked.filter(
            (path) => !paths.includes(path),
          );
          status.unstaged = status.unstaged.filter(
            (x) => !paths.includes(x.path),
          );
          return null;
        }
        case "git_unstage": {
          const paths = args.paths as string[];
          status.unstaged = [
            ...status.unstaged,
            ...status.staged.filter(
              (x) => paths.includes(x.path) && x.status !== "A",
            ),
          ];
          status.untracked = [
            ...status.untracked,
            ...status.staged
              .filter((x) => paths.includes(x.path) && x.status === "A")
              .map((x) => x.path),
          ];
          status.staged = status.staged.filter((x) => !paths.includes(x.path));
          return null;
        }
        case "git_log_page":
          return { commits: [], hasMore: false, nextCursor: null };
        case "git_log_authors":
          return [];
        case "herdr_sessions":
          return sessions;
        case "herdr_capabilities":
          return capabilities;
        case "herdr_snapshot":
          return snapshot;
        case "herdr_layout_export": {
          const id = String(args.tabId ?? "build");
          return {
            workspaceId: id === "ideas" ? "notes" : "studio",
            tabId: id,
            zoomed: false,
            focusedPaneId: `pane-${id}`,
            root: { type: "pane", paneId: `pane-${id}`, label: id, cwd: ROOT },
          };
        }
        case "herdr_tab_focus":
          snapshot.snapshot.focused_tab_id = String(args.tabId);
          return null;
        case "herdr_workspace_focus":
          snapshot.snapshot.focused_workspace_id = String(args.workspaceId);
          return null;
        case "herdr_worktree_list":
          return { repositories: [], worktrees: [] };
        case "herdr_terminal_open": {
          const id = `demo-${++nextTerminal}`;
          terminals.set(id, {
            channel: args.onEvent as Channel<unknown>,
            seq: 0,
            input: "",
            cols: Number(args.cols),
            rows: Number(args.rows),
          });
          setTimeout(() => output(id, terminalIntro, true), 40);
          return {
            sessionId: id,
            target: args.target,
            mode: "control",
            role: "controller",
            cols: args.cols,
            rows: args.rows,
            takeover: true,
          };
        }
        case "herdr_terminal_input": {
          const id = String(args.sessionId),
            entry = terminals.get(id);
          if (!entry) return null;
          for (const char of String(args.text ?? "")) {
            if (char === "\r") {
              const cmd = entry.input.trim();
              entry.input = "";
              const reply =
                cmd === "help"
                  ? "Demo commands: help, ls, git status, bun test, clear"
                  : cmd === "ls"
                    ? "README.md  src/  queries/  package.json"
                    : cmd === "git status"
                      ? "On branch feature/evening-sky\r\nChanges: src/App.tsx, src/theme.css"
                      : cmd === "bun test"
                        ? "✓ theme tokens\r\n✓ workspace navigation\r\n3 tests passed (simulated)"
                        : cmd
                          ? "Demo only — try help to see available commands."
                          : "";
              output(
                id,
                cmd === "clear" ? "\x1b[2J\x1b[H$ " : "\r\n" + reply + "\r\n$ ",
              );
            } else if (char === "\x7f") {
              entry.input = entry.input.slice(0, -1);
              output(id, "\b \b");
            } else {
              entry.input += char;
              output(id, char);
            }
          }
          return null;
        }
        case "herdr_terminal_resize": {
          const entry = terminals.get(String(args.sessionId));
          if (entry) {
            entry.cols = Number(args.cols);
            entry.rows = Number(args.rows);
          }
          return null;
        }
        case "herdr_terminal_release":
          terminals.delete(String(args.sessionId));
          return null;
        case "db_profile_list":
        case "db_profiles_load":
          return {
            profiles: [
              {
                descriptorId: dbProfile.id as DbDescriptorId,
                configGeneration: 1,
                name: dbProfile.name,
                target: { kind: "sqlite", path: dbProfile.path },
                credentialState: "notRequired",
              },
            ],
            recovery: [],
          } satisfies DbProfileLoadResult;
        case "db_list_tables":
          return dbTables;
        case "db_table_columns":
          return sampleQuery(
            `SELECT * FROM ${(args.table as { name: string }).name}`,
          ).columns.map((name) => ({
            name,
            type: "TEXT",
            notnull: true,
            pk: name === "name",
          }));
        case "db_query_run": {
          const request = args.request as DbQueryRunRequest;
          return {
            ...request,
            transactionMayBeOpen: false,
            connectionTerminated: false,
            statements: request.statements.map((statement, index) => {
              const owner = {
                descriptorId: request.descriptorId,
                connectionId: request.connectionId,
                connectionGeneration: request.connectionGeneration,
                queryRunId: request.queryRunId,
                statementExecutionId: `statement-${index}`,
                resultSessionId: `result-${index}`,
              };
              let result;
              try {
                result = sampleQuery(statement.sql);
              } catch (error) {
                return {
                  statementExecutionId: owner.statementExecutionId,
                  statementIndex: index,
                  sql: statement.sql,
                  effectOutcome: "none",
                  result: {
                    kind: "error",
                    error: {
                      engine: "sqlite",
                      message: String(error),
                      code: null,
                      position: null,
                      detail: null,
                      hint: null,
                      retryability: "notRetryable",
                    },
                  },
                };
              }
              const { columns, rows } = result;
              return {
                statementExecutionId: owner.statementExecutionId,
                statementIndex: index,
                sql: statement.sql,
                effectOutcome: "none",
                result: {
                  kind: "rows",
                  affectedRows: null,
                  resultSession: {
                    owner,
                    columns,
                    initialPage: {
                      owner,
                      pageIndex: 0,
                      columns,
                      rows,
                      hasPrevious: false,
                      hasNext: false,
                      effectOutcome: "none",
                      lifecycle: "complete",
                      resultLimitReached: false,
                    },
                  },
                },
              };
            }),
          };
        }
        case "db_result_session_release":
          return null;
        case "host_connections_list":
        case "host_profiles_load":
        case "ssh_profiles_load":
        case "trusted_workspaces_list":
          return [];
        case "log_status":
          return { enabled: false };
        case "log_list":
          return [];
        case "herdr_binary_source_get":
          return capabilities.binarySource;
        case "allow_workspace_asset_scope":
        case "start_watch":
        case "stop_watch":
        case "git_close_workspace":
        case "herdr_pane_focus":
        case "herdr_terminal_scroll":
        case "log_event":
        case "log_user_action":
          return null;
        default: {
          if (command.startsWith("plugin:")) return null;
          window.dispatchEvent(
            new CustomEvent("demo-unavailable", { detail: command }),
          );
          throw new Error(
            "This action is available in the desktop app. Demo data stays in memory.",
          );
        }
      }
    },
    { shouldMockEvents: true },
  );
}
