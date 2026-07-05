import { expect, test, it, afterEach } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import {
    openFile,
    saveFile,
    listDir,
    gitDetect,
    gitStatus,
    gitStage,
    gitUnstage,
    gitDiscard,
    gitCommit,
    gitBranches,
    gitCreateBranch,
    gitCheckout,
    gitFetch,
    gitPull,
    gitPush,
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
    lspStart,
    lspSend,
    lspStopWorkspace,
    lspStatus,
    lspConfigGet,
    lspConfigSetServer,
    lspConfigStale,
    lspConfigClearStale,
    lspSetTrace,
    lspInstallServer
} from "./ipc"
import { languageFromPath, fileGradeOf, MAX_LINE_LEN_SYNTAX_OFF } from "./types"
import type { SearchEvent, LspServerInfo, LspConfig, OpenFileResult } from "./types"

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
            return { kind: "full", content: "let a = 1", size: 9 }
        }
    })
    const r = await openFile("/w/a.ts")
    expect(r.kind).toBe("full")
    if (r.kind === "full") expect(r.content).toContain("a = 1")
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
    await gitStage(["a.ts"])
    expect(seen[0]).toEqual(["git_stage", { paths: ["a.ts"] }])
})

it("gitUnstage forwards paths", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitUnstage(["a.ts"])
    expect(seen[0]).toEqual(["git_unstage", { paths: ["a.ts"] }])
})

it("gitDiscard forwards tracked and untracked lists", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitDiscard(["a.ts"], ["b.txt"])
    expect(seen[0]).toEqual(["git_discard", { paths: ["a.ts"], untracked: ["b.txt"] }])
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

it("gitFetch forwards background flag", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitFetch(true)
    expect(seen[0]).toEqual(["git_fetch_cmd", { background: true }])
})

it("gitPull invokes git_pull_cmd", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitPull()
    expect(seen[0]).toEqual(["git_pull_cmd", {}])
})

it("gitPush invokes git_push_cmd", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitPush()
    expect(seen[0]).toEqual(["git_push_cmd", {}])
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

it("fileGradeOf returns veryLongLine for full content with an over-long line", () => {
    const result: OpenFileResult = { kind: "full", content: "", size: 0 }
    const content = "x".repeat(MAX_LINE_LEN_SYNTAX_OFF + 1)
    expect(fileGradeOf(result, content)).toBe("veryLongLine")
})

it("fileGradeOf returns full for normal full content", () => {
    const result: OpenFileResult = { kind: "full", content: "let a = 1\nlet b = 2", size: 19 }
    expect(fileGradeOf(result, "let a = 1\nlet b = 2")).toBe("full")
})

it("fileGradeOf falls back to result.content when content arg omitted", () => {
    const result: OpenFileResult = {
        kind: "full",
        content: "y".repeat(MAX_LINE_LEN_SYNTAX_OFF + 1),
        size: MAX_LINE_LEN_SYNTAX_OFF + 1
    }
    expect(fileGradeOf(result)).toBe("veryLongLine")
})

it("fileGradeOf returns the underlying kind for non-full results", () => {
    expect(fileGradeOf({ kind: "limited", content: "x", size: 1 })).toBe("limited")
    expect(fileGradeOf({ kind: "tooLarge", size: 99 })).toBe("tooLarge")
    expect(fileGradeOf({ kind: "binary", size: 99 })).toBe("binary")
    expect(fileGradeOf({ kind: "nonUtf8Readonly", content: "x", encoding: "latin1", size: 1 }))
        .toBe("nonUtf8Readonly")
})
