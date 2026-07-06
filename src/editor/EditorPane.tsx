import { useEffect, useRef, useState } from "react"
import { EditorState, StateEffect } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { buildExtensions, hasVeryLongLine } from "./cmExtensions"
import { conflictMarkers } from "./conflictMarkers"
import { getDocument, updateBuffer, documentGeneration } from "./documentRegistry"
import { registerView, unregisterView } from "./viewRegistry"
import { maybeInterceptSave } from "../workbench/ExternalChangeResolver"
import { saveFile } from "../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import { recentlySaved } from "../lib/saveSuppress"
import { useWorkspaceStore } from "../state/workspaceStore"
import { SpecialFileView } from "./SpecialFileView"
import type { OpenFileResult } from "../lib/types"
import { fileGradeOf } from "../lib/types"
import { flushPendingChanges, shouldFormatOnSave, lspExtensionsForFile } from "../lsp/lspManager"
import type { ManagedClient } from "../lsp/lspManager"
import { pathToUri } from "../lsp/workspace"
import { offsetOf } from "../lsp/diagnosticsPull"
import "./editor.css"

// Format-on-save is opt-in and default OFF (A7). The switch is persisted under
// this localStorage key; the Settings UI that writes it lands in T12 (wave 5) and
// shares this exact key. Read fresh on every save so a toggle takes effect at once.
export const FORMAT_ON_SAVE_STORAGE_KEY = "yuzora.lsp.formatOnSave.v1"

function loadFormatOnSave(): boolean {
    try {
        return localStorage.getItem(FORMAT_ON_SAVE_STORAGE_KEY) === "true"
    } catch {
        // localStorage unavailable (private mode / quota): treat as disabled.
        return false
    }
}

// Minimal structural mirror of the LSP formatting request/response we consume —
// same convention lspManager.ts / semanticTokens.ts use to avoid depending on the
// undeclared vscode-languageserver-protocol types.
interface LspPosition {
    line: number
    character: number
}
interface LspTextEdit {
    range: { start: LspPosition; end: LspPosition }
    newText: string
}
interface FormattingParams {
    textDocument: { uri: string }
    options: { tabSize: number; insertSpaces: boolean }
}

// textDocument/formatting on save, gated by capability + the user setting. Returns
// early (a resolved promise) whenever formatting is off or unavailable so the save
// path is untouched. Applied as a single transaction into the view before saveFile
// reads the doc, mirroring codeActions.applyWorkspaceEdit's offset mapping.
async function applyFormatOnSave(
    view: EditorView,
    managed: ManagedClient,
    path: string,
    isLive: () => boolean
): Promise<void> {
    if (!shouldFormatOnSave(managed.capabilities, loadFormatOnSave())) return
    // Snapshot the doc the server formats against, BEFORE the request: the returned
    // edits are offsets into THIS doc. If the user types during the await, applying
    // them against the drifted doc would silently corrupt content (offsetOf just
    // clamps), so we detect drift after the await and skip (F4).
    const docAtRequest = view.state.doc
    let edits: LspTextEdit[] | null
    try {
        edits = await managed.client.request<FormattingParams, LspTextEdit[] | null>("textDocument/formatting", {
            textDocument: { uri: pathToUri(path) },
            options: { tabSize: 4, insertSpaces: true }
        })
    } catch {
        // Server rejected / timed out — save the un-formatted text rather than block.
        return
    }
    if (!edits || edits.length === 0) return
    // View replaced/destroyed, or the doc moved on while awaiting: skip formatting
    // and save what's there — same generation-guard semantics as semanticTokens /
    // diagnosticsPull. Map against docAtRequest (identical to the current doc here).
    if (!isLive() || view.state.doc !== docAtRequest) return
    const changes = edits.map((e) => ({
        from: offsetOf(docAtRequest, e.range.start),
        to: offsetOf(docAtRequest, e.range.end),
        insert: e.newText
    }))
    // A malformed server can return an inverted range (from > to after offsetOf
    // clamps each end independently). Dispatching it throws "Invalid change range",
    // which would reject the save chain and silently drop the write. Half-applied
    // format edits also wreck indentation, so abandon the whole group and save the
    // un-formatted text — matching this function's "save rather than block" intent.
    if (changes.some((c) => c.from > c.to)) return
    try {
        view.dispatch({ changes })
    } catch (e) {
        // Defend against any other malformed-edit throw form (e.g. overlap): still
        // fall through to save the un-formatted text rather than block the save.
        console.warn("format-on-save apply failed", e)
    }
}

export function EditorPane({ path }: { path: string }) {
    const containerRef = useRef<HTMLDivElement>(null)
    const viewRef = useRef<EditorView | null>(null)
    // The ManagedClient (with capabilities) for this pane's file, set once the
    // async LSP mount resolves. Needed by save for flush + format-on-save gating.
    const managedRef = useRef<ManagedClient | null>(null)
    const [result, setResult] = useState<OpenFileResult | null>(null)
    const markDirty = useWorkspaceStore((s) => s.markDirty)
    const markExternallyModified = useWorkspaceStore((s) => s.markExternallyModified)
    const pendingReveal = useWorkspaceStore((s) => s.pendingReveal)
    const consumeReveal = useWorkspaceStore((s) => s.consumeReveal)

    // Select and center a 1-based line, clamped to the document. scrollIntoView
    // is guarded because jsdom has no layout (M1 convention).
    function revealLine(view: EditorView, n: number, focus: boolean) {
        const line = view.state.doc.line(Math.max(1, Math.min(n, view.state.doc.lines)))
        view.dispatch({ selection: { anchor: line.from, head: line.to } })
        try {
            view.dispatch({ effects: EditorView.scrollIntoView(line.from, { y: "center" }) })
        } catch {
            // no-op under jsdom
        }
        // Focus the editor so a requestReveal-driven navigation (go-to-definition,
        // find-references, symbol jump) lands the caret in the view (R3). Search-
        // result clicks pass focus:false to stay reveal-only, keeping focus on the
        // results list (A4). Guarded because jsdom's focus has no layout to act on.
        if (focus) {
            try {
                view.focus()
            } catch {
                // no-op under jsdom
            }
        }
    }

    useEffect(() => {
        let disposed = false
        const generation = documentGeneration(path)
        // Intercept clicks on links inside hover/completion tooltips (H3). sanitizeHtml
        // keeps <a href> in server markdown (and FORBID target forces in-place
        // navigation), so a click would navigate the whole Tauri webview away from the
        // SPA. Capture-phase on document so we preempt the browser's default navigation
        // for every pane. (Opening the URL in an external browser is a future task.)
        const onTooltipLinkClick = (event: MouseEvent) => {
            const target = event.target
            if (target instanceof Element && target.closest(".cm-tooltip a[href]")) {
                event.preventDefault()
            }
        }
        document.addEventListener("click", onTooltipLinkClick, true)
        void getDocument(path).then((entry) => {
            if (disposed) return
            setResult(entry.result)
            const r = entry.result
            if (r.kind === "tooLarge" || r.kind === "binary") return
            const content = r.content
            const flags = {
                readonly: r.kind === "nonUtf8Readonly",
                syntaxOff: r.kind === "limited" || hasVeryLongLine(content)
            }
            const save = () => {
                if (maybeInterceptSave(path)) return
                const view = viewRef.current
                if (!view) return
                // Flush any debounced didChange so a formatting request (and the
                // server's view of the file) sees the just-typed text (R4).
                const managed = managedRef.current
                if (managed) flushPendingChanges(managed)
                // Format-on-save needs to know this exact view is still the pane's
                // live view when its async request resolves (F4).
                const isLive = () => viewRef.current === view
                const formatted = managed
                    ? applyFormatOnSave(view, managed, path, isLive)
                    : Promise.resolve()
                void formatted
                    .then(() => {
                        recentlySaved.mark(path)
                        void saveFile(path, view.state.doc.toString())
                            .then(() => {
                                markDirty(path, false)
                                markExternallyModified(path, false)
                                void logUserAction("save_file", `save ${path}`)
                            })
                            // Save failed: dirty stays true (markDirty(false) never runs)
                            // so the tab still shows unsaved. Full error UX is out of scope.
                            .catch((e) => console.error("save failed", e))
                    })
                    .catch(() => {})
            }
            const state = EditorState.create({
                doc: content,
                extensions: [...buildExtensions(path, flags, () => markDirty(path, true), save), conflictMarkers()]
            })
            const view = new EditorView({ state, parent: containerRef.current! })
            viewRef.current = view
            registerView(path, view)
            const reveal = useWorkspaceStore.getState().pendingReveal
            if (reveal && reveal.path === path) {
                revealLine(view, reveal.line, reveal.focus ?? true)
                useWorkspaceStore.getState().consumeReveal()
            }
            // Async LSP mount (R1): the no-LSP view above is fully live first.
            // lspExtensionsForFile is the single gating source — it returns null for
            // non-full grades / unsupported types / no workspace / missing server, and
            // otherwise waits for the initialize handshake (A0) before returning the
            // ManagedClient (save needs it for flush + format gating) plus the assembled
            // extensions. They are merged with appendConfig (leaves the existing config
            // untouched, safer than reconfigure), re-checking the view wasn't
            // disposed/replaced while awaiting.
            void (async () => {
                const result = await lspExtensionsForFile(path, fileGradeOf(r, content))
                if (disposed || viewRef.current !== view || !result) return
                managedRef.current = result.managed
                view.dispatch({ effects: StateEffect.appendConfig.of(result.extensions) })
            })().catch(() => {})
        })
        return () => {
            disposed = true
            document.removeEventListener("click", onTooltipLinkClick, true)
            const view = viewRef.current
            if (view) {
                updateBuffer(path, view.state.doc.toString(), generation)
                // Pass this pane's own view so a split group that reused the path
                // isn't unregistered out from under it (m4).
                unregisterView(path, view)
            }
            view?.destroy()
            viewRef.current = null
            managedRef.current = null
        }
    }, [path, markDirty, markExternallyModified])

    // Jump to a requested line once the pane for its file is mounted. The view
    // is created asynchronously above, so a request that lands before creation
    // is handled there; this covers requests while the pane is already live.
    useEffect(() => {
        const view = viewRef.current
        if (!view || !pendingReveal || pendingReveal.path !== path) return
        revealLine(view, pendingReveal.line, pendingReveal.focus ?? true)
        consumeReveal()
    }, [pendingReveal, path, consumeReveal])

    if (result && (result.kind === "tooLarge" || result.kind === "binary")) {
        return <SpecialFileView path={path} result={result} />
    }
    return <div className="editor-pane" ref={containerRef} />
}
