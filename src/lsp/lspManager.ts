import { LSPClient, serverDiagnostics } from "@codemirror/lsp-client"
import type { Extension } from "@codemirror/state"
import type { EditorView } from "@codemirror/view"
import DOMPurify from "dompurify"

import i18n from "@/lib/i18n"
import { lspStopWorkspace } from "../lib/ipc"
import type { FileGrade, LspLanguage, LspServerInfo } from "../lib/types"
import { lspLanguageOf } from "../lib/types"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useLspStore } from "../state/lspStore"
import { assembleLspExtensions } from "./lspExtensions"
import { offsetOf } from "./diagnosticsPull"
import { createTauriTransport } from "./transport"
import type { TransportHandle } from "./transport"
import { YuzoraWorkspace, pathToUri } from "./workspace"

// Minimal structural view of the LSP ServerCapabilities we consume. The full
// type lives in vscode-languageserver-protocol, a transitive dependency we do
// not declare in package.json (W11) — and @codemirror/lsp-client does not
// re-export it — so we mirror only the field the manager reads.
export interface ServerCapabilities {
    documentFormattingProvider?: boolean | object
}

export interface ManagedClient {
    client: LSPClient
    language: LspLanguage
    capabilities: ServerCapabilities | null
}

interface FormattingPosition {
    line: number
    character: number
}

interface FormattingTextEdit {
    range: { start: FormattingPosition; end: FormattingPosition }
    newText: string
}

interface FormattingParams {
    textDocument: { uri: string }
    options: { tabSize: number; insertSpaces: boolean }
}

// LSP servers may return Markdown containing arbitrary HTML, which the client
// renders and injects for hover / completion tooltips. Without a sanitizer the
// client sets it as innerHTML verbatim — an XSS channel via a compromised or
// malicious server (W9). Route it through DOMPurify.
export function sanitizeHtml(html: string): string {
    // FORBID lists kept verbatim in sync with MarkdownPreview.tsx:31-34 — LSP
    // tooltips (hover / completion / signatureHelp) render equally-untrusted
    // server markdown through the same webview. form controls can submit-navigate
    // out of the webview; map/area are image-map nav elements; target/usemap
    // enable navigation escapes.
    return DOMPurify.sanitize(html, {
        FORBID_ATTR: ["target", "usemap", "style", "class"],
        FORBID_TAGS: ["form", "input", "button", "select", "textarea", "dialog", "map", "area", "style"]
    })
}

// One LSPClient per (workspace, language). The cache stores the in-flight
// Promise (not the resolved client) keyed identically to `handles`, so
// concurrent first calls share one startup and the server is started once.
// NUL separator so it can never collide with a path (which may contain spaces).
const SEP = "\u0000"
const keyOf = (workspace: string, language: LspLanguage) => workspace + SEP + language
const clients = new Map<string, Promise<ManagedClient | null>>()
const handles = new Map<string, TransportHandle>()

// Lazily creates (and connects) the client for a (workspace, language) pair, or
// returns the existing one. Returns null when the server is missing / crashed —
// its status is surfaced to the store for the UI, no initialize is attempted
// (that would be swallowed and dead-lock a 3s timeout), and nothing is cached so
// a later install / restart can retry.
//
// Known limitation (wave 1 handoff): the Rust side silently re-spawns a crashed
// server (< 3 times) on the same Channel without re-emitting status or
// re-initializing. The client is unaware of the respawn. Not remediated here
// (Rust is out of T6 scope); verified/handled in T15.
export function ensureClient(workspace: string, language: LspLanguage): Promise<ManagedClient | null> {
    const key = keyOf(workspace, language)
    const cached = clients.get(key)
    if (cached) return cached
    // `isCurrent` is an identity guard: every map mutation in startClient checks
    // that clients[key] is still *this* pending before acting, so a concurrent
    // stopWorkspace or a newer ensureClient (A→B→A timing) can't be clobbered and
    // no handle/client leaks (R2-4 / R2-8). The closure reads `pending`, which is
    // assigned before any post-await cleanup can run.
    let pending: Promise<ManagedClient | null>
    const isCurrent = () => clients.get(key) === pending
    pending = startClient(workspace, language, key, isCurrent)
    clients.set(key, pending)
    return pending
}

async function startClient(
    workspace: string,
    language: LspLanguage,
    key: string,
    isCurrent: () => boolean
): Promise<ManagedClient | null> {
    const handle = createTauriTransport(workspace, language)

    let info: LspServerInfo
    try {
        info = await handle.info
    } catch {
        // lspStart failed outright — don't cache the failure, allow a retry. Only
        // clear the entry if it is still ours (R2-4).
        handle.dispose()
        if (isCurrent()) clients.delete(key)
        return null
    }

    // Superseded while awaiting info (a concurrent stopWorkspace cleared us, or a
    // newer ensureClient replaced the cache entry). Abandon without touching the
    // maps or connecting — they belong to the newer owner now (R2-4 / R2-8). No
    // await follows before handles.set, so this single check covers both.
    if (!isCurrent()) {
        handle.dispose()
        return null
    }

    if (info.status.status === "missing" || info.status.status === "crashed") {
        // Surface the status so the status bar / Settings can show it, then bail
        // without connecting. Not cached: retry after install / restart. Only for
        // the workspace the UI is still on — same criterion as the LspBridge event
        // filter (R2-5).
        if (info.workspace === useWorkspaceStore.getState().workspacePath) {
            useLspStore.getState().setServerInfo(info)
        }
        handle.dispose()
        clients.delete(key)
        return null
    }

    // Record the healthy server's info so the status bar / Settings can show its
    // name — Rust only emits lsp:server-status on Crash, so this success path is
    // the sole source for a healthy server. Guard on workspace currency like the
    // missing/crashed branch (R6-2 / R2-5): a workspace the UI has left must not
    // pollute the store.
    if (info.workspace === useWorkspaceStore.getState().workspacePath) {
        useLspStore.getState().setServerInfo(info)
    }

    const client = new LSPClient({
        rootUri: pathToUri(workspace),
        sanitizeHTML: sanitizeHtml,
        workspace: (c) => new YuzoraWorkspace(c, language),
        // Push-mode diagnostics channel (F2). serverDiagnostics() is an
        // LSPClientExtension — a textDocument/publishDiagnostics notification handler
        // plus its lint rendering (with unsyncedChanges compensation) — NOT a plain
        // editor Extension, so it is wired here on the client rather than in
        // assembleLspExtensions; languageServerSupport surfaces its editorExtension
        // through LSPPlugin.create. A server that pushes diagnostics
        // (typescript-language-server) renders through this; a pull-only server
        // (rust-analyzer) is served by the gated linter in assembleLspExtensions.
        extensions: [serverDiagnostics()]
    })
    client.connect(handle.transport)

    const managed: ManagedClient = { client, language, capabilities: null }
    void client.initializing
        .then(() => {
            managed.capabilities = client.serverCapabilities
            // Mark the language ready so deriveDisplayState can resolve "ready"
            // (Rust's LspProcessStatus has no Ready variant — the completed
            // handshake is the only signal). Guard on workspace currency like
            // R2-5 so a workspace the UI has left doesn't pollute the store.
            if (info.workspace === useWorkspaceStore.getState().workspacePath) {
                useLspStore.getState().setInitialized(language, true)
            }
        })
        .catch(() => {
            // Initialize failed/timed out; capabilities stay null and initialized
            // stays unset. Status is surfaced separately via the lsp:server-status
            // event (LspBridge).
        })

    handles.set(key, handle)
    return managed
}

// Tears down every client belonging to a workspace and asks the Rust side to
// stop its processes. Called on workspace switch (LspBridge).
export function stopWorkspace(workspace: string): void {
    const prefix = workspace + SEP
    for (const key of [...clients.keys()]) {
        if (!key.startsWith(prefix)) continue
        const pending = clients.get(key)!
        void pending.then((managed) => managed?.client.disconnect()).catch(() => {})
        handles.get(key)?.dispose()
        clients.delete(key)
        handles.delete(key)
    }
    void lspStopWorkspace(workspace).catch(() => {})
}

// Format-on-save gating (A7: default OFF, opt-in via Settings — T12). Formatting
// is only offered when the server advertises documentFormattingProvider AND the
// user has enabled the setting. Pure predicate; EditorPane wires the actual
// textDocument/formatting request + apply in T10.
export function shouldFormatOnSave(capabilities: ServerCapabilities | null, enabled: boolean): boolean {
    return enabled && !!capabilities?.documentFormattingProvider
}

// Save-flush primitive: push any pending (debounced) didChange edits to the
// server before a request that must see the current document — e.g. formatting,
// so the server formats the just-typed text and not a stale version. The
// official client exposes this as LSPClient.sync() (d.ts). EditorPane calls this
// on save in T10.
export function flushPendingChanges(managed: ManagedClient): void {
    managed.client.sync()
}

// Manual formatting command used by the clicked-view context menu. Unlike the
// package's fire-and-forget command, this awaits the LSP request so a rejection
// reaches the shared action-error dialog. The document/view identity guards
// prevent applying server offsets to a replaced or concurrently edited buffer.
export async function formatEditorDocument(
    view: EditorView,
    managed: ManagedClient,
    path: string,
    isLive: () => boolean,
    syncPending = true
): Promise<boolean> {
    if (!managed.capabilities?.documentFormattingProvider) return false
    if (syncPending) flushPendingChanges(managed)
    const docAtRequest = view.state.doc
    const edits = await managed.client.request<FormattingParams, FormattingTextEdit[] | null>(
        "textDocument/formatting",
        {
            textDocument: { uri: pathToUri(path) },
            options: { tabSize: 4, insertSpaces: true }
        }
    )
    if (!edits || edits.length === 0) return true
    if (!isLive() || view.state.doc !== docAtRequest) return false

    const changes = edits.map((edit) => ({
        from: offsetOf(docAtRequest, edit.range.start),
        to: offsetOf(docAtRequest, edit.range.end),
        insert: edit.newText
    }))
    if (changes.some((change) => change.from > change.to)) {
        throw new Error(i18n.t("contextMenu.error.invalidFormattingRange", { ns: "menus" }))
    }
    view.dispatch({ changes })
    return true
}

// The single source of LSP mount gating, consumed by EditorPane. Returns the
// ManagedClient (for save flush + format-on-save gating) alongside the assembled
// editor extensions, or null when LSP must not be mounted. Non-full grades (large
// / limited / binary / very long line) return null so no didOpen is sent — the
// file gets syntax-only editing. Unsupported file types, the no-open-workspace
// case, and a missing/crashed server also return null.
export async function lspExtensionsForFile(
    path: string,
    grade: FileGrade
): Promise<{ managed: ManagedClient; extensions: Extension } | null> {
    if (grade !== "full") return null
    const language = lspLanguageOf(path)
    if (!language) return null
    const workspace = useWorkspaceStore.getState().workspacePath
    if (!workspace) return null

    const managed = await ensureClient(workspace, language)
    if (!managed) return null
    // Wait for the initialize handshake so client.serverCapabilities is populated
    // before assembling (A0). ensureClient resolves on transport connect, but a
    // cold server (rust-analyzer: 5-30s) fills capabilities only later — without
    // this the semanticTokens / diagnostics gates in assembleLspExtensions would
    // permanently see "no provider" for this view and degrade to empty. An
    // already-ready server resolves immediately; a failed handshake leaves
    // capabilities null and the gates degrade gracefully.
    await managed.client.initializing.catch(() => {})
    return { managed, extensions: assembleLspExtensions(managed, path) }
}
