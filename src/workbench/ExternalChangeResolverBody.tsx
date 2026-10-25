import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { acceptChunk, getChunks, unifiedMergeView } from "@codemirror/merge"

import { openFileSnapshot } from "../lib/ipc"
import type { ExternalChangePayload, OpenFileResult } from "../lib/types"
import { relativePathWithin, workspacePathForDisplay } from "../lib/paths"
import { logUserAction } from "@/features/logs/userAction"
import { recentlySaved } from "../lib/saveSuppress"
import { getView } from "../editor/viewRegistry"
import { documentGeneration, documentWorkspaceEpoch, getCachedDocument, reloadDocument, sameDiskSnapshot, saveDocumentContent } from "../editor/documentRegistry"
import { serializeDocumentLineEndings } from "../editor/lineEndings"
import { showMixedLineEndingSaveError } from "../editor/saveDocument"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
    dialogMinSize,
} from "../components/ui/dialog"
import { Button } from "../components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"

// A "degraded" load happens when the disk side can't be diffed: binary/tooLarge
// grades, or the file was deleted (openFile rejects). No merge view is shown —
// just a message and two coarse actions.
type Degraded = "binary" | "deleted" | null

type ResolverOwner = {
    view: EditorView
    document: EditorState["doc"]
    workspace: string | null
    epoch: number
    generation: number
}

export function ResolverBody({ path, session }: { path: string; session: number }) {
    const containerRef = useRef<HTMLDivElement>(null)
    const mergeViewRef = useRef<EditorView | null>(null)
    const bufferRef = useRef<string>("")
    // Immutable snapshot of the disk content, captured at open time and refreshed
    // on rebuild. takeDisk uses this rather than getOriginalDoc, whose `original`
    // gets pulled up to the buffer once the user runs keepAll (acceptChunk).
    const diskRef = useRef<string>("")
    const diskSnapshotRef = useRef<OpenFileResult | undefined>(undefined)
    const [degraded, setDegraded] = useState<Degraded>(null)
    const [rechanged, setRechanged] = useState(false)
    const [ready, setReady] = useState(false)
    const [saveError, setSaveError] = useState(false)
    const [saving, setSaving] = useState(false)
    const ownerRef = useRef<ResolverOwner | null>(null)
    const operationRef = useRef<object | null>(null)

    const closeResolver = useUiStore((s) => s.closeResolver)
    const markDirty = useWorkspaceStore((s) => s.markDirty)
    const markExternallyModified = useWorkspaceStore((s) => s.markExternallyModified)

    useEffect(() => {
        let disposed = false
        let readSequence = 0
        const mainView = getView(path)
        if (!mainView) {
            closeResolver()
            return
        }
        bufferRef.current = mainView.state.doc.toString()
        const owner: ResolverOwner = {
            view: mainView, document: mainView.state.doc,
            workspace: useWorkspaceStore.getState().workspacePath,
            epoch: documentWorkspaceEpoch(), generation: documentGeneration(path),
        }
        ownerRef.current = owner
        const ownerCurrent = () => !disposed && ownerRef.current === owner
            && useUiStore.getState().resolverPath === path && useUiStore.getState().resolverNonce === session
            && useWorkspaceStore.getState().workspacePath === owner.workspace
            && documentWorkspaceEpoch() === owner.epoch && documentGeneration(path) === owner.generation
            && getView(path) === mainView && mainView.state.doc === owner.document

        // Initial and watcher reads share one sequence. Accept the remote revision
        // only together with the disk content this resolver actually displays.
        function readDisk(initial = false) {
            if (!ownerCurrent()) return
            const sequence = ++readSequence
            const current = () => sequence === readSequence && ownerCurrent()
            void openFileSnapshot(path)
                .then((snapshot) => {
                    if (!current()) return
                    const parent = containerRef.current
                    if (!parent) return
                    snapshot.accept()
                    const disk = snapshot.result
                    if (sameDiskSnapshot(diskSnapshotRef.current, disk)) return
                    diskSnapshotRef.current = disk
                    if (!initial) setRechanged(true)
                    // Capture progress at acceptance, not when the read started.
                    // Keep it across text/non-text transitions as well.
                    bufferRef.current = mergeViewRef.current?.state.doc.toString() ?? bufferRef.current
                    mergeViewRef.current?.destroy()
                    mergeViewRef.current = null
                    if (disk.kind === "binary" || disk.kind === "tooLarge") {
                        setDegraded("binary")
                    } else {
                        diskRef.current = disk.content
                        setDegraded(null)
                        mergeViewRef.current = new EditorView({
                            state: EditorState.create({
                                doc: bufferRef.current,
                                extensions: [unifiedMergeView({ original: disk.content, mergeControls: true })],
                            }),
                            parent,
                        })
                    }
                    setReady(true)
                })
                .catch(() => {
                    if (!current()) return
                    if (!diskSnapshotRef.current) {
                        setDegraded("deleted")
                        setReady(true)
                    } else setRechanged(true)
                })
        }
        readDisk(true)
        const unlisten = listen<ExternalChangePayload>("fs:external-change", (event) => {
            if (event.payload.workspaceRoot !== owner.workspace || !ownerCurrent()) return
            if (event.payload.paths.some(changed => relativePathWithin(changed, path) !== null)) readDisk()
        })
        return () => {
            disposed = true
            if (ownerRef.current === owner) ownerRef.current = null
            operationRef.current = null
            void unlisten.then(fn => fn())
            mergeViewRef.current?.destroy()
            mergeViewRef.current = null
        }
    }, [path, session, closeResolver])

    function beginOperation() {
        if (operationRef.current) return
        const owner = ownerRef.current
        if (!owner) return
        const lineEnding = useWorkspaceStore.getState().getLineEnding(path)
        const cached = getCachedDocument(path)
        const cachedResult = cached?.result
        const mergeView = mergeViewRef.current
        const mergeDocument = mergeView?.state.doc
        const token = {}
        const sessionCurrent = () => ownerRef.current === owner && useUiStore.getState().resolverPath === path
            && useUiStore.getState().resolverNonce === session
        const ownerUnchanged = () => sessionCurrent()
            && documentWorkspaceEpoch() === owner.epoch
            && useWorkspaceStore.getState().workspacePath === owner.workspace
            && useWorkspaceStore.getState().getLineEnding(path) === lineEnding
            && getView(path) === owner.view && owner.view.state.doc === owner.document
            && mergeViewRef.current === mergeView && mergeView?.state.doc === mergeDocument
        const unchanged = () => ownerUnchanged() && documentGeneration(path) === owner.generation
            && getCachedDocument(path) === cached && cached?.result === cachedResult
        if (!unchanged()) { setSaveError(true); return }
        operationRef.current = token
        setSaveError(false)
        setSaving(true)
        return {
            owner, lineEnding, unchanged, ownerUnchanged,
            finish: () => {
                if (operationRef.current !== token) return
                operationRef.current = null
                if (sessionCurrent()) setSaving(false)
            },
        }
    }

    function keepAll() {
        const view = mergeViewRef.current
        if (!view) return
        // acceptChunk mutates the chunk set, so re-read after each accept and
        // stop when nothing remains.
        for (;;) {
            const chunk = getChunks(view.state)?.chunks[0]
            if (!chunk) break
            if (!acceptChunk(view, chunk.fromB)) break
        }
    }

    function takeDisk() {
        const view = mergeViewRef.current
        if (!view) return
        // Replace the whole buffer with the immutable disk snapshot in one
        // transaction. We deliberately use diskRef rather than getOriginalDoc:
        // once keepAll has run, the merge view's `original` has been pulled up to
        // the buffer, so getOriginalDoc would return the buffer, not the disk.
        view.dispatch({
            changes: { from: 0, to: view.state.doc.length, insert: diskRef.current }
        })
    }

    // Shared final commit: write `merged` to disk, mirror it into the live main
    // view as one undoable transaction, clear the dirty/external flags, log, and
    // close. Order matters for data safety — mark saveSuppress before saveFile.
    function commitMerged(merged: string) {
        const operation = beginOperation()
        if (!operation) return
        const { lineEnding } = operation
        if (!lineEnding) {
            setSaveError(true)
            operation.finish()
            return
        }
        const serialized = serializeDocumentLineEndings(merged, lineEnding)
        if (serialized.kind === "blocked") {
            void showMixedLineEndingSaveError()
            operation.finish()
            return
        }
        recentlySaved.mark(path)
        void saveDocumentContent(path, serialized.content)
            .then(() => {
                // A completed disk write does not authorize replacing a newer
                // live buffer or closing another resolver opened in the meantime.
                if (!operation.unchanged()) return
                const mainView = operation.owner.view
                mainView.dispatch({
                    changes: { from: 0, to: mainView.state.doc.length, insert: merged }
                })
                markDirty(path, false)
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `resolve ${path}`)
                closeResolver()
            })
            .catch(() => {
                // Save failed: surface the error and keep the resolver open so the
                // user can retry or cancel. Leave dirty/external flags untouched.
                if (operation.unchanged()) setSaveError(true)
            })
            .finally(operation.finish)
    }

    function resolveAndSave() {
        const view = mergeViewRef.current
        if (!view) return
        commitMerged(view.state.doc.toString())
    }

    // Degraded actions ------------------------------------------------------
    function keepMineOverwrite() {
        commitMerged(bufferRef.current)
    }

    function takeDiskReload() {
        const operation = beginOperation()
        if (!operation) return
        const cached = getCachedDocument(path)
        const disk = cached?.diskResult
        const canApply = () => operation.unchanged() && cached?.diskResult === disk
        void reloadDocument(path, canApply)
            .then((entry) => {
                // This reload itself advances the generation and replaces the
                // cache. Admit only that exact result, with the original view intact.
                if (!operation.ownerUnchanged() || getCachedDocument(path) !== entry
                    || documentGeneration(path) !== operation.owner.generation + 1) return
                // Buffer now matches disk — clear dirty as well as the external flag,
                // otherwise the tab stays marked dirty despite being reconciled (m5).
                markDirty(path, false)
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `reload ${path}`)
                closeResolver()
            })
            .catch(() => {
                if (!canApply()) return
                // R2B-F1: the file was deleted between opening the resolver and
                // clicking reload — there is nothing to reload. Settle so the
                // dialog doesn't hang: clear the external flag and close, but KEEP
                // dirty — the in-memory buffer still differs from the (absent) disk,
                // so the user can re-save to recreate the file.
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `reload-missing ${path}`)
                closeResolver()
            })
            .finally(operation.finish)
    }

    function discardAndCloseTab() {
        const s = useWorkspaceStore.getState()
        const groupIndex = s.groups.findIndex((g) => g.tabs.some((t) => t.path === path))
        if (groupIndex >= 0) s.closeTab(groupIndex, path)
        void logUserAction("resolve_external_change", `discard ${path}`)
        closeResolver()
    }

    function cancel() {
        // Cancel touches nothing: no write, no buffer change, flags untouched.
        ownerRef.current = null
        operationRef.current = null
        closeResolver()
    }

    return (
        <Dialog
            open
            onOpenChange={(open) => {
                if (!open) cancel()
            }}
        >
            <DialogContent
                resizeId="external-change"
                minSize={dialogMinSize(480, 320)}
                showCloseButton={false}
                className="flex min-h-0 flex-col gap-[12px]"
            >
                <DialogHeader>
                    <DialogTitle>檔案已被外部修改</DialogTitle>
                    <DialogDescription>
                        {degraded === "deleted"
                            ? "磁碟上的檔案已不存在，無法比對差異。"
                            : degraded === "binary"
                              ? "磁碟版無法比對差異（二進位或過大）。"
                              : workspacePathForDisplay(path)}
                    </DialogDescription>
                </DialogHeader>

                {rechanged && (
                    <div className="text-[12px] text-(--ink-3)">磁碟版已再次變更</div>
                )}

                {saveError && (
                    <div className="text-[12px] text-(--status-d)">
                        存檔失敗，請重試或取消。
                    </div>
                )}

                {degraded && (
                    <ScrollArea className="min-h-0 flex-1" focusable viewportClassName="text-[13px] text-(--ink-2)">
                        {degraded === "deleted"
                            ? "你可以保留目前編輯內容並覆寫存檔，或丟棄變更並關閉分頁。"
                            : "你可以保留目前編輯內容並覆寫存檔，或改用磁碟版重新載入。"}
                    </ScrollArea>
                )}
                <div
                    ref={containerRef}
                    hidden={!!degraded}
                    className="external-resolver-merge min-h-0 flex-1 overflow-auto rounded-[8px] border border-(--line-1)"
                />

                <DialogFooter>
                    {degraded ? (
                        <>
                            <Button variant="outline" onClick={cancel}>
                                取消
                            </Button>
                            {degraded === "deleted" ? (
                                <Button variant="outline" disabled={saving} onClick={discardAndCloseTab}>
                                    丟棄並關閉分頁
                                </Button>
                            ) : (
                                <Button variant="outline" disabled={saving} onClick={takeDiskReload}>
                                    採用磁碟版（重新載入）
                                </Button>
                            )}
                            <Button disabled={saving} onClick={keepMineOverwrite}>保留我的（覆寫存檔）</Button>
                        </>
                    ) : (
                        <>
                            <Button variant="outline" onClick={cancel}>
                                取消
                            </Button>
                            <Button variant="outline" disabled={!ready || saving} onClick={keepAll}>
                                全部保留我的
                            </Button>
                            <Button variant="outline" disabled={!ready || saving} onClick={takeDisk}>
                                全部採用磁碟版
                            </Button>
                            <Button disabled={!ready || saving} onClick={resolveAndSave}>
                                解決並存檔
                            </Button>
                        </>
                    )}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}
