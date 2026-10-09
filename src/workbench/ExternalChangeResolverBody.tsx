import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { acceptChunk, getChunks, unifiedMergeView } from "@codemirror/merge"

import { openFile } from "../lib/ipc"
import type { ExternalChangePayload, OpenFileResult } from "../lib/types"
import { relativePathWithin, workspacePathForDisplay } from "../lib/paths"
import { logUserAction } from "@/features/logs/userAction"
import { recentlySaved } from "../lib/saveSuppress"
import { getView } from "../editor/viewRegistry"
import { reloadDocument, sameDiskSnapshot, saveDocumentContent } from "../editor/documentRegistry"
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
// just a message and two coarse actions. "unreadable" is a read failure while the
// resolver is open (deleted, permission, remote host hiccup): only keep-mine is
// offered, and a later successful read restores the merge view.
type Degraded = "binary" | "deleted" | "unreadable" | null

export function ResolverBody({ path }: { path: string }) {
    const containerRef = useRef<HTMLDivElement>(null)
    const mergeViewRef = useRef<EditorView | null>(null)
    const bufferRef = useRef<string>("")
    // Immutable snapshot of the disk content, captured at open time and refreshed
    // on rebuild. takeDisk uses this rather than getOriginalDoc, whose `original`
    // gets pulled up to the buffer once the user runs keepAll (acceptChunk).
    const diskRef = useRef<string>("")
    const diskSnapshotRef = useRef<OpenFileResult | undefined>(undefined)
    // Monotonic id of the latest disk read; stale read results are ignored.
    const readGenRef = useRef(0)
    // Disk text waiting for the merge container to mount (degraded recovery).
    const pendingRestoreRef = useRef<string | null>(null)
    const [restoreTick, setRestoreTick] = useState(0)
    const [degraded, setDegraded] = useState<Degraded>(null)
    const [rechanged, setRechanged] = useState(false)
    const [ready, setReady] = useState(false)
    const [saveError, setSaveError] = useState(false)

    const closeResolver = useUiStore((s) => s.closeResolver)
    const markDirty = useWorkspaceStore((s) => s.markDirty)
    const markExternallyModified = useWorkspaceStore((s) => s.markExternallyModified)

    useEffect(() => {
        let disposed = false
        const mainView = getView(path)
        // Defensive: no live editor view for this path -> nothing safe to
        // reconcile, just close.
        if (!mainView) {
            closeResolver()
            return
        }
        const buffer = mainView.state.doc.toString()
        bufferRef.current = buffer

        const gen = ++readGenRef.current
        void openFile(path)
            .then((disk) => {
                if (disposed || gen !== readGenRef.current) return
                diskSnapshotRef.current = disk
                if (disk.kind === "binary" || disk.kind === "tooLarge") {
                    setDegraded("binary")
                    setReady(true)
                    return
                }
                diskRef.current = disk.content
                const view = new EditorView({
                    state: EditorState.create({
                        doc: buffer,
                        extensions: [
                            unifiedMergeView({ original: disk.content, mergeControls: true })
                        ]
                    }),
                    parent: containerRef.current!
                })
                mergeViewRef.current = view
                setReady(true)
            })
            .catch(() => {
                if (disposed || gen !== readGenRef.current) return
                setDegraded("deleted")
                setReady(true)
            })

        return () => {
            disposed = true
            mergeViewRef.current?.destroy()
            mergeViewRef.current = null
        }
    }, [path, closeResolver])

    // The disk side became undiffable while the resolver is open: tear down the
    // merge view so no stale-disk action (takeDisk / resolveAndSave) remains, keep
    // the user's in-progress doc as the buffer, and switch to the degraded UI.
    function enterDegraded(view: EditorView | null, next: "binary" | "unreadable") {
        if (view) {
            bufferRef.current = view.state.doc.toString()
            view.destroy()
            mergeViewRef.current = null
        }
        pendingRestoreRef.current = null
        setDegraded(next)
        setReady(true)
    }

    // The merge container only exists while not degraded, so recovery clears
    // degraded first and builds the view here once the container is mounted.
    useEffect(() => {
        const parent = containerRef.current
        const original = pendingRestoreRef.current
        if (original === null || !parent || mergeViewRef.current) return
        pendingRestoreRef.current = null
        mergeViewRef.current = new EditorView({
            state: EditorState.create({
                doc: bufferRef.current,
                extensions: [unifiedMergeView({ original, mergeControls: true })]
            }),
            parent
        })
    }, [restoreTick, degraded])

    // While open, react to further disk changes for this same path: rebuild the
    // merge view with the current in-progress doc against the fresh disk as the
    // new original, and surface a one-line hint. Rebuilding (rather than
    // patching the original in place) keeps the user's accept/reject progress
    // while guaranteeing a consistent diff against the new disk content.
    useEffect(() => {
        let disposed = false
        const unlisten = listen<ExternalChangePayload>("fs:external-change", (e) => {
            // #57 T3：先比對 live workspacePath，舊 workspace watcher 的殘留
            // 事件不得觸發 rebuild（防串場）。
            if (e.payload.workspaceRoot !== useWorkspaceStore.getState().workspacePath) return
            if (!e.payload.paths.some((changed) => relativePathWithin(changed, path) !== null)) return
            const gen = ++readGenRef.current
            void openFile(path)
                .then((disk) => {
                    if (disposed || gen !== readGenRef.current) return
                    if (sameDiskSnapshot(diskSnapshotRef.current, disk)) return
                    diskSnapshotRef.current = disk
                    setRechanged(true)
                    const view = mergeViewRef.current
                    if (disk.kind === "binary" || disk.kind === "tooLarge") {
                        enterDegraded(view, "binary")
                        return
                    }
                    diskRef.current = disk.content
                    if (!view) {
                        // Degraded (or never built): rebuild from the in-progress buffer.
                        pendingRestoreRef.current = disk.content
                        setDegraded(null)
                        setReady(true)
                        setRestoreTick((n) => n + 1)
                        return
                    }
                    const parent = containerRef.current
                    if (!parent) return
                    const doc = view.state.doc.toString()
                    view.destroy()
                    mergeViewRef.current = new EditorView({
                        state: EditorState.create({
                            doc,
                            extensions: [
                                unifiedMergeView({
                                    original: disk.content,
                                    mergeControls: true
                                })
                            ]
                        }),
                        parent
                    })
                })
                .catch(() => {
                    if (disposed || gen !== readGenRef.current) return
                    setRechanged(true)
                    // Forget the snapshot so the next successful read always restores.
                    diskSnapshotRef.current = undefined
                    enterDegraded(mergeViewRef.current, "unreadable")
                })
        })
        return () => {
            disposed = true
            void unlisten.then((fn) => fn())
        }
    }, [path])

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
        const mainView = getView(path)
        setSaveError(false)
        const lineEnding = useWorkspaceStore.getState().getLineEnding(path)
        if (!lineEnding) {
            setSaveError(true)
            return
        }
        const serialized = serializeDocumentLineEndings(merged, lineEnding)
        if (serialized.kind === "blocked") {
            void showMixedLineEndingSaveError()
            return
        }
        recentlySaved.mark(path)
        void saveDocumentContent(path, serialized.content)
            .then(() => {
                if (mainView) {
                    mainView.dispatch({
                        changes: { from: 0, to: mainView.state.doc.length, insert: merged }
                    })
                }
                markDirty(path, false)
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `resolve ${path}`)
                closeResolver()
            })
            .catch(() => {
                // Save failed: surface the error and keep the resolver open so the
                // user can retry or cancel. Leave dirty/external flags untouched.
                setSaveError(true)
            })
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
        void reloadDocument(path)
            .then(() => {
                // Buffer now matches disk — clear dirty as well as the external flag,
                // otherwise the tab stays marked dirty despite being reconciled (m5).
                markDirty(path, false)
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `reload ${path}`)
                closeResolver()
            })
            .catch(() => {
                // R2B-F1: the file was deleted between opening the resolver and
                // clicking reload — there is nothing to reload. Settle so the
                // dialog doesn't hang: clear the external flag and close, but KEEP
                // dirty — the in-memory buffer still differs from the (absent) disk,
                // so the user can re-save to recreate the file.
                markExternallyModified(path, false)
                void logUserAction("resolve_external_change", `reload-missing ${path}`)
                closeResolver()
            })
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
                            : degraded === "unreadable"
                              ? "無法讀取磁碟版本（檔案可能已刪除或暫時無法存取）。"
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

                {degraded ? (
                    <ScrollArea className="min-h-0 flex-1" focusable viewportClassName="text-[13px] text-(--ink-2)">
                        {degraded === "deleted"
                            ? "你可以保留目前編輯內容並覆寫存檔，或丟棄變更並關閉分頁。"
                            : degraded === "unreadable"
                              ? "你可以保留目前編輯內容並覆寫存檔；磁碟版本可再次讀取時會自動恢復比對。"
                              : "你可以保留目前編輯內容並覆寫存檔，或改用磁碟版重新載入。"}
                    </ScrollArea>
                ) : (
                    <div
                        ref={containerRef}
                        className="external-resolver-merge min-h-0 flex-1 overflow-auto rounded-[8px] border border-(--line-1)"
                    />
                )}

                <DialogFooter>
                    {degraded ? (
                        <>
                            <Button variant="outline" onClick={cancel}>
                                取消
                            </Button>
                            {degraded === "deleted" ? (
                                <Button variant="outline" onClick={discardAndCloseTab}>
                                    丟棄並關閉分頁
                                </Button>
                            ) : degraded === "unreadable" ? null : (
                                <Button variant="outline" onClick={takeDiskReload}>
                                    採用磁碟版（重新載入）
                                </Button>
                            )}
                            <Button onClick={keepMineOverwrite}>保留我的（覆寫存檔）</Button>
                        </>
                    ) : (
                        <>
                            <Button variant="outline" onClick={cancel}>
                                取消
                            </Button>
                            <Button variant="outline" disabled={!ready} onClick={keepAll}>
                                全部保留我的
                            </Button>
                            <Button variant="outline" disabled={!ready} onClick={takeDisk}>
                                全部採用磁碟版
                            </Button>
                            <Button disabled={!ready} onClick={resolveAndSave}>
                                解決並存檔
                            </Button>
                        </>
                    )}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}
