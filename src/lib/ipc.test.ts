import { expect, test, it, afterEach, describe } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import * as ipcModule from "./ipc"
import {
    openFile,
    isOpenableFile,
    saveFile,
    listDir,
    gitBootstrap,
    workspaceTrustStatus,
    workspaceTrustGrant,
    workspaceTrustList,
    workspaceTrustRevoke,
    gitStatus,
    gitStage,
    gitUnstage,
    gitDiscard,
    gitRollbackPaths,
    gitCommit,
    gitBranches,
    gitCreateBranch,
    gitCheckout,
    gitCheckoutDetached,
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
    dbPostgresTransportChallenge,
    dbListTables,
    dbTableColumns,
    dbQueryRun,
    dbQueryCancel,
    dbResultPagePrevious,
    dbResultPageNext,
    dbResultSessionRelease,
    sftpPickSelectedPath,
    sftpPickDownloadDestination,
    sftpUpload,
    sftpDownload,
    sshHostKeyRespond
} from "./ipc"
import { languageFromPath, fileGradeOf, MAX_LINE_LEN_SYNTAX_OFF } from "./types"
import type { SearchEvent, OpenFileResult } from "./types"
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

test("isOpenableFile probes a path without loading file contents", async () => {
    mockIPC((cmd, args) => {
        expect(cmd).toBe("is_openable_file")
        expect(args).toEqual({ path: "/w/a.ts" })
        return true
    })
    await expect(isOpenableFile("/w/a.ts")).resolves.toBe(true)
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

test("sshHostKeyRespond forwards the bound first-use decision", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        return undefined
    })
    await sshHostKeyRespond("chal-1", true, "example.com:22", "SHA256:abc")
    expect(seen).toEqual([
        [
            "ssh_host_key_respond",
            {
                challengeId: "chal-1",
                accept: true,
                endpoint: "example.com:22",
                fingerprint: "SHA256:abc"
            }
        ]
    ])
})

test("sftp upload/download send tagged source and dest dir + leaf", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        if (cmd === "sftp_pick_selected_path") return [{ id: "sel-1", leaf: "picked.txt" }]
        if (cmd === "sftp_pick_download_destination") return { id: "download-1", leaf: "a.txt" }
        return undefined
    })
    await expect(sftpPickSelectedPath()).resolves.toEqual([{ id: "sel-1", leaf: "picked.txt" }])
    await expect(sftpPickDownloadDestination("a.txt")).resolves.toEqual({
        id: "download-1",
        leaf: "a.txt"
    })
    await sftpUpload(
        "sess-1",
        "xfer-1",
        { kind: "workspace", workspaceId: "ws-opaque", relativePath: "a.txt" },
        "/home/u"
    )
    await sftpDownload("sess-1", "xfer-2", "/home/u/a.txt", {
        capabilityId: "download-1",
        leaf: "a.txt"
    })
    expect(seen).toEqual([
        ["sftp_pick_selected_path", {}],
        ["sftp_pick_download_destination", { suggestedLeaf: "a.txt" }],
        [
            "sftp_upload",
            {
                sessionId: "sess-1",
                request: {
                    transferId: "xfer-1",
                    source: { kind: "workspace", workspaceId: "ws-opaque", relativePath: "a.txt" },
                    remoteDir: "/home/u",
                    expectedRevision: null
                }
            }
        ],
        [
            "sftp_download",
            {
                sessionId: "sess-1",
                transferId: "xfer-2",
                remotePath: "/home/u/a.txt",
                destinationCapabilityId: "download-1"
            }
        ]
    ])
})

test("languageFromPath 依副檔名判斷", () => {
    expect(languageFromPath("/a/b.ts")).toBe("TypeScript")
    expect(languageFromPath("/a/b.rs")).toBe("Rust")
    expect(languageFromPath("/a/b.unknown")).toBe("Plain Text")
})

// #57 T3：git 面板首載單趟完成——bootstrap 一次回齊 environment＋status＋branches
// （Ready 落地後快照失敗時 status/branches 為 null、錯誤走 snapshotError）。
it("gitBootstrap forwards path and returns the one-trip snapshot", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_bootstrap")
        expect((payload as { path: string }).path).toBe("/w")
        return { environment: { status: "notARepo" }, status: null, branches: null, snapshotError: null }
    })
    const result = await gitBootstrap("/w")
    expect(result).toEqual({
        environment: { status: "notARepo" },
        status: null,
        branches: null,
        snapshotError: null
    })
})

it("workspace trust commands forward challenge payloads", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => {
        seen.push([cmd, payload])
        if (cmd === "workspace_trust_status") {
            return { state: "untrusted", challengeId: "c1", canonicalPath: "/w", repoPresent: true }
        }
        if (cmd === "workspace_trust_list") {
            return [{ canonicalPath: "/w", fsIdentity: "id", grantedAt: "2026-01-01T00:00:00Z" }]
        }
        if (cmd === "workspace_trust_revoke") {
            return []
        }
        return { state: "trusted", canonicalPath: "/w" }
    })
    await expect(workspaceTrustStatus("/w")).resolves.toMatchObject({ state: "untrusted" })
    await expect(workspaceTrustGrant("c1")).resolves.toMatchObject({ state: "trusted" })
    await expect(workspaceTrustList()).resolves.toEqual([
        { canonicalPath: "/w", fsIdentity: "id", grantedAt: "2026-01-01T00:00:00Z" }
    ])
    await expect(workspaceTrustRevoke("/w")).resolves.toEqual([])
    expect(seen).toEqual([
        ["workspace_trust_status", { path: "/w" }],
        ["workspace_trust_grant", { challengeId: "c1" }],
        ["workspace_trust_list", {}],
        ["workspace_trust_revoke", { canonicalPath: "/w" }]
    ])
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
    const s = await gitStatus("/w", ["src/a.ts"])
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
    const s = await gitStatus("/w")
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
    await gitCommit("/w", "wip")
    expect(seen[0]).toEqual(["git_commit_cmd", { repositoryRoot: "/w", message: "wip" }])
})

it("gitCommit forwards the expected HEAD for amend without staging or pushing", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    const head = "a".repeat(40)
    await gitCommit("/w", "updated message", head)
    expect(seen).toEqual([["git_commit_cmd", { repositoryRoot: "/w", message: "updated message", amendHead: head }]])
})

it("gitBranches returns branch list", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_branches")
        return { local: [], remote: [], tags: [{ name: "v1", date: "2026-08-01T12:00:00Z" }] }
    })
    const b = await gitBranches("/w")
    expect(b).toEqual({ local: [], remote: [], tags: [{ name: "v1", date: "2026-08-01T12:00:00Z" }] })
})

it("gitCreateBranch forwards optional exact start point", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCreateBranch("/w", "feat/x")
    await gitCreateBranch("/w", "release/x", "origin/team/release/x")
    expect(seen[0]).toEqual(["git_create_branch", { repositoryRoot: "/w", name: "feat/x", startPoint: null }])
    expect(seen[1]).toEqual(["git_create_branch", {
        repositoryRoot: "/w",
        name: "release/x",
        startPoint: "origin/team/release/x"
    }])
})

it("gitCheckout forwards name", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCheckout("/w", "main")
    expect(seen[0]).toEqual(["git_checkout", { repositoryRoot: "/w", name: "main" }])
})

it("gitCheckoutDetached forwards the exact revision to the explicit command", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitCheckoutDetached("/w", "release/v1.0.0")
    expect(seen[0]).toEqual([
        "git_checkout_detached",
        { repositoryRoot: "/w", rev: "release/v1.0.0" }
    ])
})

it("gitFetch forwards background flag and optional repository authority", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitFetch("/repo", true)
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
    await gitCherryPick("/w", "deadbeef")
    expect(seen[0]).toEqual(["git_cherry_pick", { repositoryRoot: "/w", hash: "deadbeef" }])
})

it("gitRemoteProbe returns probe result", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_remote_probe")
        return "yes"
    })
    expect(await gitRemoteProbe("/w")).toBe("yes")
})

it("gitDiffContent forwards path and staged flag", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_diff_content")
        expect(payload).toEqual({ repositoryRoot: "/w", path: "a.ts", staged: true, origPath: null })
        return { original: { kind: "binary" }, modified: { kind: "full", content: "x" } }
    })
    const d = await gitDiffContent("/w", "a.ts", true)
    expect(d.modified.kind).toBe("full")
})

it("gitConflictAbort forwards op", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitConflictAbort("/w", "merge")
    expect(seen[0]).toEqual(["git_conflict_abort", { repositoryRoot: "/w", op: "merge" }])
})

it("gitConflictContinue forwards op", async () => {
    const seen: unknown[] = []
    mockIPC((cmd, payload) => { seen.push([cmd, payload]) })
    await gitConflictContinue("/w", "rebase")
    expect(seen[0]).toEqual(["git_conflict_continue", { repositoryRoot: "/w", op: "rebase" }])
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
            repositoryRoot: "/w",
            cursor: "cursor-200",
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
            hasMore: true,
            nextCursor: "cursor-400"
        }
    })
    const p = await gitLogPage("/w", "cursor-200", 200, "fix", "Alice")
    expect(p.hasMore).toBe(true)
    expect(p.commits[0].refs[0].kind).toBe("local")
})

it("gitLogPage defaults optional filters to null", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_log_page")
        expect(payload).toEqual({ repositoryRoot: "/w", cursor: null, limit: 200, query: null, author: null, since: null, until: null })
        return { commits: [], hasMore: false, nextCursor: null }
    })
    const p = await gitLogPage("/w", null, 200)
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
    const d = await gitCommitDetail("/w", "deadbeef")
    expect(d.files[0].path).toBe("a.ts")
    expect(d.totalAdditions).toBe(2)
})

it("gitLogAuthors returns author entries", async () => {
    mockIPC((cmd) => {
        expect(cmd).toBe("git_log_authors")
        return [{ name: "Alice", email: "a@x" }]
    })
    const authors = await gitLogAuthors("/w")
    expect(authors).toEqual([{ name: "Alice", email: "a@x" }])
})

it("gitFileAtRev forwards rev and path and returns tagged union", async () => {
    mockIPC((cmd, payload) => {
        expect(cmd).toBe("git_file_at_rev")
        expect(payload).toEqual({ repositoryRoot: "/w", rev: "HEAD", path: "src/a.ts" })
        return { kind: "full", content: "let a = 1" }
    })
    const r = await gitFileAtRev("/w", "HEAD", "src/a.ts")
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
        transportMode: "verifyFull" as const,
        insecureException: null,
        trustServerCertAcknowledged: false
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
            "sqliteOpenFailed",
            "postgresTransportRejected"
        ]
        expect(codes).toHaveLength(12)
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

    it("forwards a PostgreSQL transport challenge request", async () => {
        const seen: unknown[] = []
        const request = {
            transportMode: "insecurePlaintext" as const,
            host: "db.internal",
            port: 5432,
            user: "alice",
            database: "app"
        }
        const issued = {
            challengeId: "pg-chal-1",
            ...request,
            expiresAt: 1_700_000_000_000
        }
        mockIPC((cmd, payload) => {
            seen.push([cmd, payload])
            if (cmd === "db_postgres_transport_challenge") return issued
        })
        await expect(dbPostgresTransportChallenge(request)).resolves.toEqual(issued)
        expect(seen).toEqual([["db_postgres_transport_challenge", { request }]])
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
