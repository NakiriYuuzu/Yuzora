import { expect, test, it, afterEach, describe } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import * as ipcModule from "./ipc"
import {
    openFile,
    saveFile,
    listDir,
    workspacePathIndex,
    gitDetect,
    gitStatus,
    gitStage,
    gitUnstage,
    gitDiscard,
    gitRollbackPaths,
    gitCommit,
    gitBranches,
    gitCreateBranch,
    gitCheckout,
    gitFetch,
    gitPull,
    gitPush,
    gitCherryPick,
    gitRemoteProbe,
    gitDiffContent,
    gitConflictAbort,
    gitConflictContinue,
    askpassRespond,
    searchWorkspace,
    gitLogPage,
    gitCommitDetail,
    gitLogAuthors,
    gitFileAtRev,
    ptyOpen,
    ptyWrite,
    ptyResize,
    ptyClose,
    ptyCloseWorkspace,
    devServerDetect,
    devServerStart,
    devServerStop,
    devServerStopWorkspace,
    lspStart,
    lspSend,
    lspStopWorkspace,
    lspStatus,
    lspConfigGet,
    lspConfigSetServer,
    lspConfigStale,
    lspConfigClearStale,
    lspSetTrace,
    lspInstallServer,
    agentKill,
    agentStderrTail,
    dbProfileList,
    dbProfileImportLegacy,
    dbProfileCreate,
    dbProfileUpdate,
    dbProfileRemoveCredential,
    dbProfileForget,
    dbProfileRecover,
    dbProfileOpen,
    dbProfileDisconnect,
    dbTestConnection,
    dbListTables,
    dbTableColumns,
    dbQueryRun,
    dbQueryCancel,
    dbResultPagePrevious,
    dbResultPageNext,
    dbResultSessionRelease
} from "./ipc"
import { languageFromPath, fileGradeOf, MAX_LINE_LEN_SYNTAX_OFF } from "./types"
import type {
    SearchEvent,
    LspServerInfo,
    LspConfig,
    OpenFileResult,
    PtyEvent,
    DevServerStatus
} from "./types"
import type {
    DbConnectionGeneration,
    DbConnectionId,
    DbDescriptorId,
    DbError,
    DbLiveConnection,
    DbOperationalError,
    DbProfileDescriptor,
    DbProfileErrorCode,
    DbProfileLoadResult,
    DbProfileRecoveryRequest,
    DbQueryRunRequest,
    DbQueryRunId,
    DbResultPage,
    DbResultSessionId,
    DbSaveAndConnectOutcome,
    DbStatementExecutionId
} from "./types"

const sampleServerInfo: LspServerInfo = {
    workspace: "/w",
    language: "typescript",
    serverId: "typescript-language-server",
    command: "typescript-language-server --stdio",
    path: "/usr/bin/typescript-language-server",
    status: { status: "starting" },
    lastStartupLog: null,
    lastError: null,
    restartCount: 0
}

afterEach(() => clearMocks())

test("openFile 傳遞 path 並回傳分級結果", async () => {
    mockIPC((cmd, args) => {
        if (cmd === "open_file") {
            expect((args as { path: string }).path).toBe("/w/a.ts")
            return { kind: "full", content: "let a = 1", size: 9, lineEnding: "crlf" }
        }
    })
    const r = await openFile("/w/a.ts")
    expect(r.kind).toBe("full")
    if (r.kind === "full") {
        expect(r.content).toContain("a = 1")
        expect(r.lineEnding).toBe("crlf")
    }
})

test("saveFile 回傳 mtime", async () => {
    mockIPC((cmd) => (cmd === "save_file" ? 1234 : undefined))
    expect(await saveFile("/w/a.ts", "x")).toBe(1234)
})

test("listDir 回傳節點", async () => {
    mockIPC((cmd) =>
        cmd === "list_dir" ? [{ name: "src", path: "/w/src", isDir: true }] : undefined
    )
    const nodes = await listDir("/w")
    expect(nodes[0].isDir).toBe(true)
})

test("workspacePathIndex uses a typed request/response without a search channel", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return {
            workspace: "/w",
            entries: [{ relativePath: "src/a.ts", canonicalPath: "/w/src/a.ts" }],
            truncated: false
        }
    })

    await expect(workspacePathIndex("/w")).resolves.toEqual({
        workspace: "/w",
        entries: [{ relativePath: "src/a.ts", canonicalPath: "/w/src/a.ts" }],
        truncated: false
    })
    expect(seen).toEqual([["workspace_path_index", { workspace: "/w" }]])
})

test("languageFromPath 依副檔名判斷", () => {
    expect(languageFromPath("/a/b.ts")).toBe("TypeScript")
    expect(languageFromPath("/a/b.rs")).toBe("Rust")
    expect(languageFromPath("/a/b.unknown")).toBe("Plain Text")
})

it("gitDetect forwards path and returns environment", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_detect")
        expect((payload as { path: string }).path).toBe("/w")
        return { status: "ready", root: "/w", version: "2.40.0" }
    })
    const env = await gitDetect("/w")
    expect(env).toEqual({ status: "ready", root: "/w", version: "2.40.0" })
})

it("gitStatus forwards pathspec and returns status", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_status_cmd")
        expect((payload as { pathspec: string[] | null }).pathspec).toEqual(["src/a.ts"])
        return {
            branch: "main", headOid: "x", detached: false, upstream: null, ahead: 0, behind: 0,
            staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null
        }
    })
    const s = await gitStatus(["src/a.ts"])
    expect(s.branch).toBe("main")
})

it("gitStatus defaults pathspec to null", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_status_cmd")
        expect((payload as { pathspec: string[] | null }).pathspec).toBeNull()
        return {
            branch: null, headOid: "", detached: false, upstream: null, ahead: 0, behind: 0,
            staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null
        }
    })
    const s = await gitStatus()
    expect(s.branch).toBeNull()
})

it("gitStage forwards paths", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitStage("/w", ["a.ts"])
    expect(seen[0]).toEqual(["git_stage", { repositoryRoot: "/w", paths: ["a.ts"] }])
})

it("gitUnstage forwards paths", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitUnstage("/w", ["a.ts"])
    expect(seen[0]).toEqual(["git_unstage", { repositoryRoot: "/w", paths: ["a.ts"] }])
})

it("gitDiscard forwards tracked and untracked lists", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitDiscard("/w", ["a.ts"], ["b.txt"])
    expect(seen[0]).toEqual([
        "git_discard",
        { repositoryRoot: "/w", paths: ["a.ts"], untracked: ["b.txt"] }
    ])
})

it("gitRollbackPaths forwards deduplicated status snapshots and explicit delete opt-in", async () => {
    const targets = [
        {
            path: "renamed.ts",
            classification: {
                kind: "tracked" as const,
                stagedStatus: "R",
                unstagedStatus: null,
                origPath: "original.ts"
            }
        }
    ]
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_rollback_paths")
        expect(payload).toEqual({
            repositoryRoot: "/w",
            targets,
            deleteUntrackedOrAdded: true
        })
        return { restored: ["renamed.ts"], preservedUntracked: [], deleted: [] }
    })

    await expect(gitRollbackPaths("/w", targets, true)).resolves.toEqual({
        restored: ["renamed.ts"],
        preservedUntracked: [],
        deleted: []
    })
})

it("gitCommit forwards message", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCommit("wip")
    expect(seen[0]).toEqual(["git_commit_cmd", { message: "wip" }])
})

it("gitBranches returns branch list", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_branches")
        return { local: [], remote: [] }
    })
    const b = await gitBranches()
    expect(b).toEqual({ local: [], remote: [] })
})

it("gitCreateBranch forwards name", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCreateBranch("feat/x")
    expect(seen[0]).toEqual(["git_create_branch", { name: "feat/x" }])
})

it("gitCheckout forwards name", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCheckout("main")
    expect(seen[0]).toEqual(["git_checkout", { name: "main" }])
})

it("gitFetch forwards background flag and optional repository authority", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitFetch(true, "/repo")
    expect(seen[0]).toEqual(["git_fetch_cmd", { background: true, repositoryRoot: "/repo" }])
})

it("gitPull forwards optional repository authority", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitPull("/repo")
    expect(seen[0]).toEqual(["git_pull_cmd", { repositoryRoot: "/repo" }])
})

it("gitPush forwards optional repository authority", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitPush("/repo")
    expect(seen[0]).toEqual(["git_push_cmd", { repositoryRoot: "/repo" }])
})

it("gitCherryPick forwards hash", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCherryPick("deadbeef")
    expect(seen[0]).toEqual(["git_cherry_pick", { hash: "deadbeef" }])
})

it("gitRemoteProbe returns probe result", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_remote_probe")
        return "yes"
    })
    expect(await gitRemoteProbe()).toBe("yes")
})

it("gitDiffContent forwards path and staged flag", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_diff_content")
        expect(payload).toEqual({ path: "a.ts", staged: true })
        return { original: { kind: "binary" }, modified: { kind: "full", content: "x" } }
    })
    const d = await gitDiffContent("a.ts", true)
    expect(d.modified.kind).toBe("full")
})

it("gitConflictAbort forwards op", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitConflictAbort("merge")
    expect(seen[0]).toEqual(["git_conflict_abort", { op: "merge" }])
})

it("gitConflictContinue forwards op", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitConflictContinue("rebase")
    expect(seen[0]).toEqual(["git_conflict_continue", { op: "rebase" }])
})

it("askpassRespond forwards id and response", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await askpassRespond(7, null)
    expect(seen[0]).toEqual(["askpass_respond", { id: 7, response: null }])
})

it("gitLogPage forwards paging + filter args and returns a page", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_log_page")
        expect(payload).toEqual({
            skip: 200,
            limit: 200,
            query: "fix",
            author: "Alice",
            since: null,
            until: null
        })
        return {
            commits: [
                {
                    hash: "abc",
                    shortHash: "abc",
                    subject: "s",
                    authorName: "Alice",
                    authorEmail: "a@x",
                    timestamp: 1700000000,
                    parents: [],
                    refs: [{ name: "main", kind: "local" }]
                }
            ],
            hasMore: true
        }
    })
    const p = await gitLogPage(200, 200, "fix", "Alice")
    expect(p.hasMore).toBe(true)
    expect(p.commits[0].refs[0].kind).toBe("local")
})

it("gitLogPage defaults optional filters to null", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_log_page")
        expect(payload).toEqual({ skip: 0, limit: 200, query: null, author: null, since: null, until: null })
        return { commits: [], hasMore: false }
    })
    const p = await gitLogPage(0, 200)
    expect(p.commits).toEqual([])
})

it("gitCommitDetail forwards hash and returns detail", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_commit_detail")
        expect((payload as { hash: string }).hash).toBe("deadbeef")
        return {
            subject: "s", body: "b", authorName: "Alice", authorEmail: "a@x",
            timestamp: 1700000000, parents: ["p1"],
            files: [{ status: "M", path: "a.ts", oldPath: null, additions: 2, deletions: 1, binary: false }],
            totalAdditions: 2, totalDeletions: 1
        }
    })
    const d = await gitCommitDetail("deadbeef")
    expect(d.files[0].path).toBe("a.ts")
    expect(d.totalAdditions).toBe(2)
})

it("gitLogAuthors returns author entries", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_log_authors")
        return [{ name: "Alice", email: "a@x" }]
    })
    const authors = await gitLogAuthors()
    expect(authors).toEqual([{ name: "Alice", email: "a@x" }])
})

it("gitFileAtRev forwards rev and path and returns tagged union", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_file_at_rev")
        expect(payload).toEqual({ rev: "HEAD", path: "src/a.ts" })
        return { kind: "full", content: "let a = 1" }
    })
    const r = await gitFileAtRev("HEAD", "src/a.ts")
    expect(r.kind).toBe("full")
    if (r.kind === "full") expect(r.content).toContain("a = 1")
})

it("searchWorkspace forwards args and streams channel events", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("search_workspace")
        const p = payload as {
            root: string
            query: string
            caseSensitive: boolean
            onEvent: { onmessage: (e: SearchEvent) => void }
        }
        expect(p.root).toBe("/w")
        expect(p.query).toBe("q")
        expect(p.caseSensitive).toBe(false)
        p.onEvent.onmessage({ type: "done", truncated: false, fileCount: 0 })
    })
    const events: SearchEvent[] = []
    await searchWorkspace("/w", "q", false, (e) => events.push(e))
    expect(events).toEqual([{ type: "done", truncated: false, fileCount: 0 }])
})

it("lspStart forwards args, wires channel, and returns server info", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("lsp_start")
        const p = payload as {
            workspace: string
            language: string
            onMessage: { onmessage: (msg: string) => void }
        }
        expect(p.workspace).toBe("/w")
        expect(p.language).toBe("typescript")
        p.onMessage.onmessage("{}")
        return sampleServerInfo
    })
    const msgs: string[] = []
    const info = await lspStart("/w", "typescript", (m) => msgs.push(m))
    expect(msgs).toEqual(["{}"])
    expect(info.serverId).toBe("typescript-language-server")
})

it("ptyOpen forwards args, wires channel, and returns session info", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("pty_open")
        const p = payload as {
            workspace: string
            sessionId: string
            shell: string | null
            shellArgs: string[] | undefined
            cols: number
            rows: number
            onEvent: { onmessage: (event: PtyEvent) => void }
        }
        expect(p.workspace).toBe("/w")
        expect(p.sessionId).toBe("pty-1")
        expect(p.shell).toBeNull()
        expect(p.shellArgs).toEqual(["-c", "echo ok"])
        expect(p.cols).toBe(120)
        expect(p.rows).toBe(32)
        p.onEvent.onmessage({ type: "output", data: "ready\n" })
        return { sessionId: "pty-1", workspace: "/w", shell: "/bin/zsh", cols: 120, rows: 32 }
    })
    const events: PtyEvent[] = []
    const info = await ptyOpen("/w", "pty-1", null, ["-c", "echo ok"], 120, 32, (event) =>
        events.push(event)
    )
    expect(events).toEqual([{ type: "output", data: "ready\n" }])
    expect(info.sessionId).toBe("pty-1")
})

it("ptyWrite forwards session id and data", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await ptyWrite("pty-1", "pwd\n")
    expect(seen[0]).toEqual(["pty_write", { sessionId: "pty-1", data: "pwd\n" }])
})

it("ptyResize forwards dimensions", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await ptyResize("pty-1", 100, 28)
    expect(seen[0]).toEqual(["pty_resize", { sessionId: "pty-1", cols: 100, rows: 28 }])
})

it("ptyClose forwards session id", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await ptyClose("pty-1")
    expect(seen[0]).toEqual(["pty_close", { sessionId: "pty-1" }])
})

it("ptyCloseWorkspace forwards workspace", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await ptyCloseWorkspace("/w")
    expect(seen[0]).toEqual(["pty_close_workspace", { workspace: "/w" }])
})

it("devServerDetect forwards workspace and extra ports then returns candidates", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("dev_server_detect")
        expect(payload).toEqual({ workspace: "/w", extraPorts: [6000] })
        return {
            candidates: [{ scriptName: "dev", command: "vite", likelyPort: 5173 }],
            runningPorts: [5173]
        }
    })
    const detect = await devServerDetect("/w", [6000])
    expect(detect.candidates[0].scriptName).toBe("dev")
    expect(detect.runningPorts).toEqual([5173])
})

it("devServerStart forwards args, wires channel, and returns server info", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("dev_server_start")
        const p = payload as {
            workspace: string
            command: string
            port: number | null
            onOutput: { onmessage: (line: string) => void }
        }
        expect(p.workspace).toBe("/w")
        expect(p.command).toBe("bun run dev")
        expect(p.port).toBeNull()
        p.onOutput.onmessage("Local: http://localhost:5173")
        return {
            workspace: "/w",
            command: "bun run dev",
            port: null,
            status: { status: "starting" }
        }
    })
    const lines: string[] = []
    const info = await devServerStart("/w", "bun run dev", null, (line) => lines.push(line))
    expect(lines).toEqual(["Local: http://localhost:5173"])
    expect(info.status.status).toBe("starting")
})

it("devServerStop forwards workspace", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await devServerStop("/w")
    expect(seen[0]).toEqual(["dev_server_stop", { workspace: "/w" }])
})

it("devServerStopWorkspace forwards workspace", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await devServerStopWorkspace("/w")
    expect(seen[0]).toEqual(["dev_server_stop_workspace", { workspace: "/w" }])
})

it("narrows pty and dev-server discriminated unions", () => {
    function ptyText(event: PtyEvent): string {
        if (event.type === "output") return event.data
        return String(event.code ?? "none")
    }
    function serverText(status: DevServerStatus): string {
        if (status.status === "running") return String(status.port ?? "auto")
        if (status.status === "failed") return status.reason
        if (status.status === "exited") return String(status.code ?? "none")
        return status.status
    }

    expect(ptyText({ type: "output", data: "ok" })).toBe("ok")
    expect(ptyText({ type: "exit", code: null })).toBe("none")
    expect(serverText({ status: "running", port: 5173 })).toBe("5173")
    expect(serverText({ status: "failed", reason: "missing script" })).toBe("missing script")
    expect(serverText({ status: "exited", code: null })).toBe("none")
    expect(serverText({ status: "starting" })).toBe("starting")
})

it("lspSend forwards workspace, language and message", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await lspSend("/w", "python", "{\"jsonrpc\":\"2.0\"}")
    expect(seen[0]).toEqual([
        "lsp_send",
        { workspace: "/w", language: "python", message: "{\"jsonrpc\":\"2.0\"}" }
    ])
})

it("lspStopWorkspace forwards workspace", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await lspStopWorkspace("/w")
    expect(seen[0]).toEqual(["lsp_stop_workspace", { workspace: "/w" }])
})

it("lspStatus forwards workspace and returns server list", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("lsp_status")
        expect((payload as { workspace: string }).workspace).toBe("/w")
        return [sampleServerInfo]
    })
    const list = await lspStatus("/w")
    expect(list[0].language).toBe("typescript")
})

it("lspConfigGet returns config", async () => {
    const config: LspConfig = {
        defaults: { typescript: "typescript-language-server" },
        workspaces: { "/w": { python: "pyright" } }
    }
    mockIPC((cmd) => {
        expect(cmd).toBe("lsp_config_get")
        return config
    })
    const c = await lspConfigGet()
    expect(c.defaults.typescript).toBe("typescript-language-server")
})

it("lspConfigSetServer forwards workspace, language and serverId", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return { defaults: {}, workspaces: {} }
    })
    await lspConfigSetServer("/w", "rust", "rust-analyzer")
    expect(seen[0]).toEqual([
        "lsp_config_set_server",
        { workspace: "/w", language: "rust", serverId: "rust-analyzer" }
    ])
})

it("lspConfigSetServer forwards null workspace for defaults", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return { defaults: {}, workspaces: {} }
    })
    await lspConfigSetServer(null, "rust", "rust-analyzer")
    expect(seen[0]).toEqual([
        "lsp_config_set_server",
        { workspace: null, language: "rust", serverId: "rust-analyzer" }
    ])
})

it("lspConfigStale returns stale workspace list", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("lsp_config_stale")
        return ["/w"]
    })
    expect(await lspConfigStale()).toEqual(["/w"])
})

it("lspConfigClearStale forwards workspace", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return { defaults: {}, workspaces: {} }
    })
    await lspConfigClearStale("/w")
    expect(seen[0]).toEqual(["lsp_config_clear_stale", { workspace: "/w" }])
})

it("lspSetTrace forwards enabled flag", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await lspSetTrace(true)
    expect(seen[0]).toEqual(["lsp_set_trace", { enabled: true }])
})

it("lspInstallServer forwards workspace and language and returns server info", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return sampleServerInfo
    })
    const info = await lspInstallServer("/w", "typescript")
    expect(seen[0]).toEqual(["lsp_install_server", { workspace: "/w", language: "typescript" }])
    expect(info.serverId).toBe("typescript-language-server")
})

it("lspInstallServer forwards null workspace for a global install", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return sampleServerInfo
    })
    await lspInstallServer(null, "python")
    expect(seen[0]).toEqual(["lsp_install_server", { workspace: null, language: "python" }])
})

it("agentKill forwards id and reason, defaulting reason to null", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await agentKill("agent-1", "user_stop")
    await agentKill("agent-2")
    expect(seen[0]).toEqual(["agent_kill", { id: "agent-1", reason: "user_stop" }])
    expect(seen[1]).toEqual(["agent_kill", { id: "agent-2", reason: null }])
})

it("agentStderrTail forwards id and returns the stderr tail", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("agent_stderr_tail")
        expect((payload as { id: string }).id).toBe("agent-1")
        return ["boom", "EPIPE"]
    })
    expect(await agentStderrTail("agent-1")).toEqual(["boom", "EPIPE"])
})

it("fileGradeOf returns veryLongLine for full content with an over-long line", () => {
    const result: OpenFileResult = { kind: "full", content: "", size: 0, lineEnding: "lf" }
    const content = "x".repeat(MAX_LINE_LEN_SYNTAX_OFF + 1)
    expect(fileGradeOf(result, content)).toBe("veryLongLine")
})

it("fileGradeOf returns full for normal full content", () => {
    const result: OpenFileResult = {
        kind: "full",
        content: "let a = 1\nlet b = 2",
        size: 19,
        lineEnding: "lf"
    }
    expect(fileGradeOf(result, "let a = 1\nlet b = 2")).toBe("full")
})

it("fileGradeOf falls back to result.content when content arg omitted", () => {
    const result: OpenFileResult = {
        kind: "full",
        content: "y".repeat(MAX_LINE_LEN_SYNTAX_OFF + 1),
        size: MAX_LINE_LEN_SYNTAX_OFF + 1,
        lineEnding: "lf"
    }
    expect(fileGradeOf(result)).toBe("veryLongLine")
})

it("fileGradeOf returns the underlying kind for non-full results", () => {
    expect(fileGradeOf({ kind: "limited", content: "x", size: 1, lineEnding: "lf" }))
        .toBe("limited")
    expect(fileGradeOf({ kind: "tooLarge", size: 99 })).toBe("tooLarge")
    expect(fileGradeOf({ kind: "binary", size: 99 })).toBe("binary")
    expect(fileGradeOf({ kind: "nonUtf8Readonly", content: "x", encoding: "latin1", size: 1 }))
        .toBe("nonUtf8Readonly")
})

describe("database v2 IPC contract seams", () => {
    const descriptorId = "descriptor-1" as DbDescriptorId
    const connectionId = "connection-1" as DbConnectionId
    const connectionGeneration = "generation-7" as DbConnectionGeneration
    const queryRunId = "query-run-1" as DbQueryRunId
    const statementExecutionId = "statement-1" as DbStatementExecutionId
    const resultSessionId = "result-session-1" as DbResultSessionId
    const identity = { descriptorId, connectionId, connectionGeneration }
    const queryOwner = { ...identity, queryRunId }
    const resultOwner = { ...queryOwner, statementExecutionId, resultSessionId }
    const target = {
        kind: "postgres" as const,
        host: "db.internal",
        port: 5432,
        database: "app",
        user: "alice",
        ssl: true,
        trustCert: false
    }
    const profile: DbProfileDescriptor = {
        descriptorId,
        configGeneration: 4,
        name: "App",
        target,
        credentialState: "stored"
    }
    const loadResult: DbProfileLoadResult = {
        profiles: [profile],
        recovery: [{
            operationId: "operation-1",
            descriptorId,
            kind: "pendingReplace",
            allowedActions: ["resume", "abort"]
        }]
    }

    it("keeps live engines separate from local structured-error provenance", () => {
        const live: DbLiveConnection = { ...identity, engine: "sqlite" }
        const localError: DbError = {
            engine: "yuzora",
            message: "local validation failed",
            code: null,
            position: null,
            detail: null,
            hint: null,
            retryability: "notRetryable"
        }
        expect(live.engine).toBe("sqlite")
        expect(localError.engine).toBe("yuzora")
    })

    it("preserves an operational recovery code with optional engine diagnostics", () => {
        const error: DbOperationalError = {
            code: "queryFailed",
            message: "database query failed",
            error: {
                engine: "postgres",
                message: "syntax error",
                code: "42601",
                position: { offset: 9, line: null, column: null },
                detail: "near FROM",
                hint: "check the select list",
                retryability: "notRetryable"
            }
        }
        expect(error.code).toBe("queryFailed")
        expect(error.error).toMatchObject({
            engine: "postgres",
            code: "42601",
            position: { offset: 9 }
        })
    })

    it("forwards metadata requests with exact live ownership", async () => {
        const seen: unknown[] = []
        mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
        const table = {
            catalog: "app",
            schema: "public",
            name: "orders",
            kind: "table" as const
        }

        await dbListTables(identity)
        await dbTableColumns(identity, table)

        expect(seen).toEqual([
            ["db_list_tables", { identity }],
            ["db_table_columns", { identity, table }]
        ])
    })

    it("keeps P3 operational error codes stable and exhaustive at the frontend boundary", () => {
        const codes: DbProfileErrorCode[] = [
            "connectionFailed",
            "connectionBusy",
            "serverDisconnected",
            "metadataFailed",
            "queryFailed",
            "staleConnection",
            "sqlitePathMissing",
            "sqlitePathNotFile",
            "sqlitePathUnreadable",
            "sqlitePathInvalid",
            "sqliteOpenFailed"
        ]
        expect(codes).toHaveLength(11)
        expect(profile.configGeneration).toBe(4)
    })

    it("forwards profile list/import/create/update lifecycle requests and returns exact results", async () => {
        const seen: unknown[] = []
        const connected: DbSaveAndConnectOutcome = {
            outcome: "connected",
            profile,
            connection: { ...identity, engine: "postgres" }
        }
        const updated = { ...profile, name: "App 2" }
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (cmd === "db_profile_list" || cmd === "db_profile_import_legacy") return loadResult
            if (cmd === "db_profile_create") return connected
            if (cmd === "db_profile_update") return updated
        })
        const listed = await dbProfileList()
        const imported = await dbProfileImportLegacy({ profiles: [profile] })
        const created = await dbProfileCreate({
            name: "App",
            target,
            credential: { password: "write-only" }
        })
        const updateRequest = {
            descriptorId,
            name: "App 2",
            target,
            replacementCredential: null
        }
        const updateResult = await dbProfileUpdate(updateRequest)
        expect(listed).toEqual(loadResult)
        expect(imported).toEqual(loadResult)
        expect(created).toEqual(connected)
        expect(updateResult).toEqual(updated)
        expect(seen).toEqual([
            ["db_profile_list", {}],
            ["db_profile_import_legacy", { request: { profiles: [profile] } }],
            ["db_profile_create", {
                request: { name: "App", target, credential: { password: "write-only" } }
            }],
            ["db_profile_update", { request: updateRequest }]
        ])
    })

    it("returns saved-but-connect-failed as a tagged create outcome", async () => {
        const outcome: DbSaveAndConnectOutcome = {
            outcome: "savedButConnectFailed",
            profile,
            error: { code: "connectionFailed", message: "database connection failed" }
        }
        mockIPC((cmd) => (cmd === "db_profile_create" ? outcome : undefined))
        await expect(dbProfileCreate({ name: "App", target, credential: null }))
            .resolves.toEqual(outcome)
    })

    it("forwards recovery/removal/forget/open/disconnect and returns exact results", async () => {
        const seen: unknown[] = []
        const noRecovery: DbProfileLoadResult = { profiles: [profile], recovery: [] }
        const connection = { ...identity, engine: "postgres" as const }
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (
                cmd === "db_profile_remove_credential"
                || cmd === "db_profile_forget"
                || cmd === "db_profile_recover"
            ) return noRecovery
            if (cmd === "db_profile_open") return connection
        })
        const removed = await dbProfileRemoveCredential(descriptorId)
        const forgotten = await dbProfileForget(descriptorId)
        const recoverRequest: DbProfileRecoveryRequest = {
            operationId: "operation-1",
            action: "retryCleanup",
            credential: null
        }
        const recovered = await dbProfileRecover(recoverRequest)
        const opened = await dbProfileOpen(descriptorId)
        await dbProfileDisconnect(identity)
        expect(removed).toEqual(noRecovery)
        expect(forgotten).toEqual(noRecovery)
        expect(recovered).toEqual(noRecovery)
        expect(opened).toEqual(connection)
        expect(seen).toEqual([
            ["db_profile_remove_credential", { descriptorId }],
            ["db_profile_forget", { descriptorId }],
            ["db_profile_recover", { request: recoverRequest }],
            ["db_profile_open", { descriptorId }],
            ["db_profile_disconnect", { identity }]
        ])
    })

    it("forwards a test connection request and returns the backend probe result", async () => {
        const seen: unknown[] = []
        const result = { elapsedMs: 42, serverVersion: "16.3" }
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (cmd === "db_test_connection") return result
        })
        const request = { kind: "ephemeral" as const, target, credential: { password: "probe" } }
        await expect(dbTestConnection(request)).resolves.toEqual(result)
        expect(seen).toEqual([["db_test_connection", { request }]])
    })

    it("exposes the exact profile wrapper inventory without credential readback", () => {
        const wrappers = Object.keys(ipcModule)
            .filter((name) => name.startsWith("dbProfile"))
            .sort()

        expect(wrappers).toEqual([
            "dbProfileCreate",
            "dbProfileDisconnect",
            "dbProfileForget",
            "dbProfileImportLegacy",
            "dbProfileList",
            "dbProfileOpen",
            "dbProfileRecover",
            "dbProfileRemoveCredential",
            "dbProfileUpdate"
        ])
        expect(Object.keys(ipcModule).filter((name) => /credential/i.test(name)))
            .toEqual(["dbProfileRemoveCredential"])
        expect("dbOpen" in ipcModule).toBe(false)
        expect("dbClose" in ipcModule).toBe(false)
    })

    it("forwards an ordered query run and exact generation-bound cancellation", async () => {
        const seen: unknown[] = []
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (cmd === "db_query_cancel") return { outcome: "cancelledConnectionTerminated" }
        })
        const request = {
            ...queryOwner,
            mode: "script",
            statements: [
                { sql: "BEGIN;", transactionBoundary: "begin" },
                { sql: "UPDATE t SET n = 2", transactionBoundary: "none" }
            ]
        } satisfies DbQueryRunRequest
        await dbQueryRun(request)
        const cancelled = await dbQueryCancel(queryOwner)
        expect(cancelled).toEqual({ outcome: "cancelledConnectionTerminated" })
        expect(seen).toEqual([
            ["db_query_run", { request }],
            ["db_query_cancel", { owner: queryOwner }]
        ])
    })

    it("forwards previous/next/release on one exact result owner and returns wire pages", async () => {
        const seen: unknown[] = []
        const page = (
            pageIndex: number,
            lifecycle: DbResultPage["lifecycle"]
        ): DbResultPage => ({
            owner: resultOwner,
            pageIndex,
            columns: ["value"],
            rows: [[{ kind: "integer", value: String(pageIndex + 1) }]],
            hasPrevious: pageIndex > 0,
            hasNext: lifecycle === "streaming",
            effectOutcome: lifecycle === "released" ? "committed" : "transactionPending",
            lifecycle,
            resultLimitReached: false
        })
        const previousPage = page(0, "streaming")
        const nextPage = page(1, "streaming")
        const releasedPage = page(1, "released")
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (cmd === "db_result_page") {
                const direction = (payload as { request: { direction: string } }).request.direction
                return direction === "previous" ? previousPage : nextPage
            }
            if (cmd === "db_result_session_release") return releasedPage
        })

        await expect(dbResultPagePrevious(resultOwner)).resolves.toEqual(previousPage)
        await expect(dbResultPageNext(resultOwner)).resolves.toEqual(nextPage)
        await expect(dbResultSessionRelease(resultOwner)).resolves.toEqual(releasedPage)
        expect(seen).toEqual([
            ["db_result_page", { request: { owner: resultOwner, direction: "previous" } }],
            ["db_result_page", { request: { owner: resultOwner, direction: "next" } }],
            ["db_result_session_release", { owner: resultOwner }]
        ])
    })
})
