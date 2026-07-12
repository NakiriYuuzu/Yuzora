import { ChangeSet, Text } from "@codemirror/state"
import type { TransactionSpec } from "@codemirror/state"
import type { EditorView } from "@codemirror/view"
import { LSPPlugin, Workspace } from "@codemirror/lsp-client"
import type { LSPClient, WorkspaceFile } from "@codemirror/lsp-client"

import { openFile, saveFile } from "../lib/ipc"
import { recentlySaved } from "../lib/saveSuppress"
import { fileGradeOf } from "../lib/types"
import type { LspLanguage } from "../lib/types"
import { getView } from "../editor/viewRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"

// --- path <-> file URI ---------------------------------------------------
// Yuzora tracks files by absolute filesystem path; LSP tracks them by URI.
export function pathToUri(path: string): string {
    let normalized = path.replace(/\\/g, "/")
    if (normalized.toLowerCase().startsWith("//?/unc/")) {
        normalized = `//${normalized.slice("//?/UNC/".length)}`
    } else if (normalized.startsWith("//?/")) {
        normalized = normalized.slice("//?/".length)
    }
    if (normalized.startsWith("//")) {
        const [host = "", ...segments] = normalized.slice(2).split("/")
        return `file://${encodeURIComponent(host)}/${segments.map(encodeURIComponent).join("/")}`
    }
    const windowsDrive = normalized.match(/^([A-Za-z]):(?:\/(.*))?$/)
    if (windowsDrive) {
        const drive = windowsDrive[1].toUpperCase()
        const rest = (windowsDrive[2] ?? "").split("/").map(encodeURIComponent).join("/")
        return `file:///${drive}:/${rest}`
    }
    return "file://" + normalized.split("/").map(encodeURIComponent).join("/")
}

export function uriToPath(uri: string): string {
    const hasFileScheme = uri.startsWith("file://")
    const schemeBody = hasFileScheme ? uri.slice("file://".length) : uri
    const body = hasFileScheme && schemeBody && !schemeBody.startsWith("/")
        ? `//${schemeBody}`
        : schemeBody
    // Keep this total: a server may hand back an unencoded URI with a bare '%'
    // (e.g. file:///ws/100%done.md), on which decodeURIComponent throws. Falling
    // back to the raw body means callers (requestFile / displayFile) can never be
    // made to reject by a malformed URI (R7-1).
    try {
        const decoded = decodeURIComponent(body)
        return /^\/[A-Za-z]:\//.test(decoded) ? decoded.slice(1) : decoded
    } catch {
        return body
    }
}

// Structural mirror of the library's (non-exported) WorkspaceFileUpdate, so the
// syncFiles override's return type stays assignable to the abstract signature.
interface FileUpdate {
    file: WorkspaceFile
    prevDoc: Text
    changes: ChangeSet
}

// A file tracked by the workspace. Its view is resolved dynamically from the
// editor's view registry, so a file that starts life as a background document
// (no editor) transparently "upgrades" to editor-backed once a pane mounts —
// getView() then returns the live view and every delegate path follows it.
class YuzoraFile implements WorkspaceFile {
    constructor(
        public uri: string,
        public languageId: string,
        public version: number,
        public doc: Text
    ) {}

    getView(): EditorView | null {
        return getView(uriToPath(this.uri)) ?? null
    }
}

// Cap on background (non-editor) documents kept resident. Evicting the LRU
// entry sends didClose so the server drops it too.
const LRU_LIMIT = 20

// YuzoraWorkspace replaces the library's DefaultWorkspace (which only knows
// files that have an active editor) so the client can service cross-file
// requests — go-to-definition, find-references, rename — into files the user
// has never opened (A1, 2026-07-03).
//
// Background edits keep the server in sync: after an off-editor rename edit is
// applied and saved to disk, a full-document didChange (monotonic version) is
// sent so the server's copy matches disk. A later editor open then reuses the
// already-tracked file, keeping editor / disk / server all consistent (W6).
//
// Known limitation (wave 1 handoff): displayFile opens a real tab but the
// EditorPane view is created asynchronously by React, so the returned view may
// be null on the first turn. Cross-file jump selection is finalized in T10 /
// verified in T15.
export class YuzoraWorkspace extends Workspace {
    files: WorkspaceFile[] = []
    private bgOrder: string[] = []
    private fileVersions: Record<string, number> = Object.create(null)
    // Per-uri in-flight background load, so concurrent requestFile calls for the
    // same file (library find-references issues them via Promise.all) share one
    // openFile / didOpen and produce exactly one entry (W1).
    private loading = new Map<string, Promise<WorkspaceFile | null>>()

    constructor(client: LSPClient, private language: LspLanguage) {
        super(client)
    }

    private nextFileVersion(uri: string): number {
        const next = (this.fileVersions[uri] ?? -1) + 1
        this.fileVersions[uri] = next
        return next
    }

    private removeFromBgOrder(uri: string) {
        const i = this.bgOrder.indexOf(uri)
        if (i >= 0) this.bgOrder.splice(i, 1)
    }

    private evictIfNeeded() {
        while (this.bgOrder.length > LRU_LIMIT) {
            const uri = this.bgOrder.shift()!
            const file = this.getFile(uri)
            // A promoted (editor-backed) file left LRU tracking already; skip.
            if (!file || file.getView()) continue
            this.files = this.files.filter((f) => f.uri !== uri)
            this.client.didClose(uri)
        }
    }

    // Cross-file request pointing at a possibly-unopened file. Background-load
    // its content via the existing openFile ipc, register a background document
    // and didOpen it. Non-full grades (large / limited / binary / very long
    // line) are not loaded — matching the editor's LSP mount guard.
    async requestFile(uri: string): Promise<WorkspaceFile | null> {
        const existing = this.getFile(uri)
        if (existing) {
            const i = this.bgOrder.indexOf(uri)
            if (i >= 0) {
                this.bgOrder.splice(i, 1)
                this.bgOrder.push(uri)
            }
            return existing
        }

        const pending = this.loading.get(uri)
        if (pending) return pending
        const load = this.loadBackground(uri)
        this.loading.set(uri, load)
        try {
            return await load
        } finally {
            this.loading.delete(uri)
        }
    }

    private async loadBackground(uri: string): Promise<WorkspaceFile | null> {
        const path = uriToPath(uri)
        // requestFile must never reject (DefaultWorkspace's contract — the library
        // fans out find-references via Promise.all with no per-item catch, so a
        // reject would wipe every reference and leak an unhandled rejection). A
        // deleted / moved / unreadable file degrades to null like a non-full grade
        // (R2-1). The in-flight map is cleared by requestFile's finally either way.
        let result
        try {
            result = await openFile(path)
        } catch {
            return null
        }
        if (result.kind !== "full") return null
        if (fileGradeOf(result, result.content) !== "full") return null

        // An editor may have opened the file while we awaited; don't double-track.
        const raced = this.getFile(uri)
        if (raced) return raced

        // Seed the version through nextFileVersion so didOpen is v0 and the first
        // didChange is v1 — monotonic even after the file is promoted to an
        // editor (W2). Hard-coding 0 desynced the version counter.
        const file = new YuzoraFile(uri, this.language, this.nextFileVersion(uri), Text.of(result.content.split("\n")))
        this.files.push(file)
        this.bgOrder.push(uri)
        this.client.didOpen(file)
        this.evictIfNeeded()
        return file
    }

    syncFiles(): readonly FileUpdate[] {
        const result: FileUpdate[] = []
        for (const file of this.files) {
            const view = file.getView()
            if (!view) continue
            const plugin = LSPPlugin.get(view)
            if (!plugin) continue
            const changes = plugin.unsyncedChanges
            if (changes.empty) continue
            result.push({ changes, file, prevDoc: file.doc })
            file.doc = view.state.doc
            file.version = this.nextFileVersion(file.uri)
            plugin.clear()
        }
        return result
    }

    // Called by the LSP plugin when an editor for `uri` mounts (T10). If the
    // file is already tracked (e.g. background-loaded), drop it from LRU
    // tracking — it is now editor-backed and its view is the source of truth.
    openFile(uri: string, languageId: string, view: EditorView): void {
        if (this.getFile(uri)) {
            this.removeFromBgOrder(uri)
            return
        }
        const file = new YuzoraFile(uri, languageId, this.nextFileVersion(uri), view.state.doc)
        this.files.push(file)
        this.client.didOpen(file)
    }

    closeFile(uri: string, _view?: EditorView): void {
        if (!this.getFile(uri)) return
        this.removeFromBgOrder(uri)
        this.files = this.files.filter((f) => f.uri !== uri)
        this.client.didClose(uri)
    }

    // Server-initiated change (e.g. rename / WorkspaceEdit). If the file is open
    // in an editor, the view's buffer is the single source of truth — dispatch
    // into it and never touch disk. For a background file, apply the edit to the
    // resident doc and write it back, marking recentlySaved first so the watcher
    // doesn't flag it as an external change.
    updateFile(uri: string, update: TransactionSpec): void {
        const file = this.getFile(uri)
        if (!file) return

        const view = file.getView()
        if (view) {
            view.dispatch(update)
            return
        }

        if (update.changes == null) return
        const changes = ChangeSet.of(update.changes, file.doc.length)
        const newDoc = changes.apply(file.doc)
        const content = newDoc.toString()
        file.doc = newDoc
        file.version = this.nextFileVersion(uri)

        // Keep the server in sync with the edit we just applied off-editor: send a
        // full-document didChange at the new (monotonic) version. Without this the
        // server keeps the pre-edit text and a later editor open would desync it
        // (W6). A no-range contentChange is a full replacement under both Full and
        // Incremental sync.
        this.client.notification("textDocument/didChange", {
            textDocument: { uri, version: file.version },
            contentChanges: [{ text: content }]
        })

        const path = uriToPath(uri)
        recentlySaved.mark(path)
        // Best-effort writeback: a disk failure (read-only / deleted / disk full)
        // must not become an unhandled rejection (R6-1, same as the other
        // fire-and-forget IPCs).
        void saveFile(path, content).catch(() => {})
    }

    // User navigation into a (possibly unopened) file: open a real tab. The view
    // is created asynchronously by EditorPane, so it may not be resolvable yet.
    async displayFile(uri: string): Promise<EditorView | null> {
        const path = uriToPath(uri)
        useWorkspaceStore.getState().openTab(path)
        return getView(path) ?? null
    }
}
