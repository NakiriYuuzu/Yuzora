import { mockIPC } from "@tauri-apps/api/mocks"
import { installDemoRuntime, ROOT, files, status, branches, environment } from "./runtime"
import type { LogCommit } from "@/lib/types"

// Separate dev entry: all invokes delegate only to the installed memory demo.
installDemoRuntime()
const memoryInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke
const originalStatus = structuredClone(status)
const originalBranches = structuredClone(branches)
const oid = (number: number) => number.toString(16).padStart(40, "0")
let serial = 100
const commitMessages = new Map<string, string>([[oid(100), "Memory commit subject\n\nMemory commit body"]])
let delay = 0
let failBranches = false
let failFetch = false
let failBranchesAfterFetch = false
let automaticWatcherRetry = false
let longDiff = false
const originalAppSource = files["src/App.tsx"]
const longSource = (side: string) => Array.from({ length: 600 }, (_, index) =>
    `// ${side} line ${index + 1} ${index % 30 === 0 ? "horizontal-content-".repeat(45) : "sample"}`
).join("\n") + `\n// ${side} END MARKER\n`
status.headOid = oid(serial)
const commits: LogCommit[] = [
    { hash: oid(100), shortHash: "0000064", subject: "Memory merge: 32 branches", authorName: "Demo", authorEmail: "demo@example.test", timestamp: 1789000000, parents: Array.from({ length: 32 }, (_, i) => oid(i + 1)), refs: [{ name: "feature/evening-sky", kind: "local" }] },
    ...Array.from({ length: 32 }, (_, i): LogCommit => ({ hash: oid(i + 1), shortHash: (i + 1).toString(16).padStart(7, "0"), subject: `Memory branch ${i + 1}`, authorName: "Demo", authorEmail: "demo@example.test", timestamp: 1788999999 - i, parents: [oid(99)], refs: [{ name: `branch-${i + 1}`, kind: "local" }] })),
    { hash: oid(99), shortHash: "0000063", subject: "Shared root", authorName: "Demo", authorEmail: "demo@example.test", timestamp: 1788999900, parents: [], refs: [] },
]
const log = document.createElement("pre")
log.setAttribute("data-testid", "memory-git-commands")
log.style.cssText = "max-height:130px;overflow:auto;white-space:pre-wrap;margin:4px;font:11px monospace"
const record = (command: string, args: unknown) => { log.textContent = `${command} ${JSON.stringify(args)}\n${log.textContent ?? ""}`.slice(0, 16000) }
const pause = () => new Promise(resolve => setTimeout(resolve, delay))

mockIPC(async (command, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>
    if (command.startsWith("git_")) record(command, args)
    switch (command) {
        case "git_bootstrap": return { environment, status: structuredClone(status), branches: structuredClone(branches) }
        case "git_status_cmd": await pause(); return structuredClone(status)
        case "git_branches":
            if (failBranches) {
                failBranches = false
                if (automaticWatcherRetry) {
                    automaticWatcherRetry = false
                    setTimeout(() => {
                        record("WATCHER retry during pending snapshot", { busy: useGitStore.getState().busy, stale: useGitStore.getState().snapshotStale })
                        void useGitStore.getState().loadBranches()
                    }, 500)
                }
                throw new Error("MEMORY: one-shot branch read failure")
            }
            return structuredClone(branches)
        case "git_fetch_cmd":
            if (failFetch) { failFetch = false; throw new Error("MEMORY: authentication failed") }
            if (failBranchesAfterFetch) { failBranches = true; failBranchesAfterFetch = false }
            return null
        case "git_pull_cmd": case "git_push_cmd": return null
        case "git_create_branch":
            branches.local.push({ name: String(args.name), upstream: "", ahead: 0, behind: 0, isCurrent: false, gone: false }); return null
        case "git_checkout":
            status.branch = String(args.name)
            branches.local.forEach(branch => { branch.isCurrent = branch.name === status.branch })
            return null
        case "git_commit_cmd":
            if (args.amendHead && args.amendHead !== status.headOid) throw new Error("MEMORY: HEAD changed")
            status.headOid = oid(++serial)
            commitMessages.set(status.headOid, String(args.message))
            status.staged = []
            return null
        case "git_log_page": return { commits: structuredClone(commits), hasMore: false, nextCursor: null }
        case "git_log_authors": return [{ name: "Demo", email: "demo@example.test" }]
        case "git_commit_detail": {
            const message = commitMessages.get(String(args.hash)) ?? "Memory commit subject\n\nMemory commit body"
            const [subject, ...body] = message.split("\n")
            return {
            subject, body: body.join("\n").replace(/^\n/, ""), authorName: "Demo", authorEmail: "demo@example.test", timestamp: 1789000000, parents: [oid(99)],
            files: [{ path: "src/App.tsx", oldPath: null, status: "M", additions: 3, deletions: 1, binary: false }], totalAdditions: 3, totalDeletions: 1,
        }
        }
        case "git_file_at_rev": return { kind: "full", content: files[String(args.path)] ?? "Memory historical file\n" }
        case "git_diff_content":
            if (longDiff && String(args.path).endsWith("src/App.tsx")) return {
                original: { kind: "full", content: longSource("ORIGINAL") },
                modified: { kind: "full", content: longSource("MODIFIED") },
            }
            break
        case "git_stage": case "git_unstage": case "git_remote_probe": case "git_detect": case "git_close_workspace": break
        default: if (command.startsWith("git_")) throw new Error(`MEMORY: unsupported ${command}`)
    }
    return memoryInvoke(command, payload)
}, { shouldMockEvents: true })

const { mountDemo } = await import("./Demo")
await mountDemo()
const { useGitStore } = await import("@/state/gitStore")
await useGitStore.getState().detect(ROOT)
const tools = document.createElement("details")
tools.open = true
tools.style.cssText = "position:fixed;bottom:28px;left:12px;z-index:99999;max-width:650px;background:#fff;color:#111;border:2px solid #267a35;padding:5px"
const title = document.createElement("summary")
title.textContent = "Git acceptance · MEMORY ONLY · collapse for app controls"
tools.append(title)
function button(label: string, action: () => void) {
    const element = document.createElement("button")
    element.textContent = label
    element.style.cssText = "margin:3px;padding:4px;border:1px solid #777"
    element.onclick = action
    tools.append(element)
}
button("Success", () => { delay = 0; failBranches = false; failFetch = false; failBranchesAfterFetch = false; automaticWatcherRetry = false; record("SCENARIO success", {}) })
button("Slow snapshot 2s", () => { delay = 2000; record("SCENARIO slow", {}) })
button("Fail branches once", () => { failBranches = true; record("SCENARIO branches fail once", {}) })
button("Fail branches after Fetch", () => { failBranchesAfterFetch = true; record("SCENARIO branches fail after fetch", {}) })
button("Fetch failure + auto watcher recovery", () => {
    delay = 2000; failBranchesAfterFetch = true; automaticWatcherRetry = true
    record("SCENARIO fetch branch failure then watcher recovery at 500ms", {})
})
button("Use long diff", () => {
    longDiff = true
    files["src/App.tsx"] = longSource("MODIFIED")
    record("SCENARIO long diff (close and reopen App.tsx diff)", {})
})
button("Watcher retry branches", () => {
    record("WATCHER manual retry", { busy: useGitStore.getState().busy, stale: useGitStore.getState().snapshotStale })
    void useGitStore.getState().loadBranches()
})
button("Fail fetch once", () => { failFetch = true; record("SCENARIO fetch fail once", {}) })
button("Reset Git memory", () => {
    Object.assign(status, structuredClone(originalStatus), { headOid: oid(100) })
    Object.assign(branches, structuredClone(originalBranches))
    commitMessages.clear(); commitMessages.set(oid(100), "Memory commit subject\n\nMemory commit body")
    delay = 0; failBranches = false; failFetch = false; failBranchesAfterFetch = false; automaticWatcherRetry = false; longDiff = false
    files["src/App.tsx"] = originalAppSource
    void useGitStore.getState().detect(ROOT)
})
button("Clear command log", () => { log.textContent = "" })
tools.append(log)
document.body.append(tools)
