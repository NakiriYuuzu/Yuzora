import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { syntaxHighlighting } from "@codemirror/language"
import { EditorState, RangeSet, RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state"
import { Decoration, EditorView, WidgetType, lineNumbers, type DecorationSet } from "@codemirror/view"
import { ChevronDown, ChevronUp, Wand2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
    dialogMinSize
} from "@/components/ui/dialog"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { hasVeryLongLine, languageExtensions } from "@/editor/cmExtensions"
import { logUserAction } from "@/features/logs/userAction"
import { gitConflictSides, gitStage, saveFile } from "@/lib/ipc"
import { isWindowsPath, nativePathJoin } from "@/lib/paths"
import { dirtyTabPaths } from "@/lib/unsavedGuard"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { mergeRegions, resolveSimpleConflict, splitLines, type MergeRegion, type MergeRegionKind } from "./mergeModel"
import { conflictMarkerCount, mergeTexts, readableText, type MergeTexts } from "./mergeTexts"

type RegionAction = "ours" | "theirs" | "both" | "ignore"

interface ResultRegion {
    from: number
    to: number
    resolved: boolean
}

const setRegion = StateEffect.define<{ index: number } & ResultRegion>()

function offsets(lines: readonly string[]): number[] {
    const out = [0]
    for (const line of lines) out.push(out[out.length - 1] + line.length)
    return out
}

const regionClass: Record<MergeRegionKind, string> = {
    ours: "cm-merge-ours",
    theirs: "cm-merge-theirs",
    both: "cm-merge-both",
    conflict: "cm-merge-conflict"
}

const mergeTheme = EditorView.baseTheme({
    ".cm-merge-ours": { backgroundColor: "rgba(59, 111, 224, 0.14)" },
    ".cm-merge-theirs": { backgroundColor: "rgba(34, 150, 88, 0.15)" },
    ".cm-merge-both": { backgroundColor: "rgba(128, 128, 128, 0.14)" },
    ".cm-merge-conflict": { backgroundColor: "rgba(214, 48, 72, 0.16)" },
    ".cm-merge-resolved": { backgroundColor: "rgba(128, 128, 128, 0.07)" },
    ".cm-merge-actions": {
        display: "flex",
        flexWrap: "wrap",
        gap: "4px",
        padding: "2px 6px",
        fontSize: "11px",
        fontFamily: "var(--font-sans, system-ui)"
    },
    ".cm-merge-actions button": {
        border: "1px solid var(--line-1, #ccc)",
        borderRadius: "5px",
        padding: "0 6px",
        background: "var(--paper-0, #fff)",
        color: "var(--ink-1, #222)",
        cursor: "pointer"
    },
    ".cm-merge-actions button:hover": { background: "var(--yz-hover, #eee)" }
})

function lineDecorations(state: EditorState, ranges: Array<{ from: number; to: number; className: string }>): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>()
    const lines: Array<{ at: number; className: string }> = []
    for (const range of ranges) {
        if (range.to <= range.from) continue
        const first = state.doc.lineAt(range.from).number
        const last = state.doc.lineAt(Math.max(range.from, range.to - 1)).number
        for (let n = first; n <= last; n++) lines.push({ at: state.doc.line(n).from, className: range.className })
    }
    lines.sort((a, b) => a.at - b.at)
    for (const line of lines) builder.add(line.at, line.at, Decoration.line({ class: line.className }))
    return builder.finish()
}

/** Read-only side pane with its regions highlighted. */
function sideExtension(regions: readonly MergeRegion[], side: "ours" | "theirs", lineOffsets: readonly number[]): Extension {
    return StateField.define<DecorationSet>({
        create: (state) => lineDecorations(state, regions.map((region) => ({
            from: lineOffsets[region[side][0]],
            to: lineOffsets[region[side][1]],
            className: regionClass[region.kind]
        }))),
        update: (value) => value,
        provide: (field) => EditorView.decorations.from(field)
    })
}

class RegionActionsWidget extends WidgetType {
    constructor(
        readonly index: number,
        readonly kind: MergeRegionKind,
        readonly labels: Record<RegionAction, string>,
        readonly act: (index: number, action: RegionAction) => void
    ) {
        super()
    }
    eq(other: RegionActionsWidget) {
        return other.index === this.index && other.kind === this.kind
    }
    toDOM() {
        const wrap = document.createElement("div")
        wrap.className = "cm-merge-actions"
        wrap.dataset.mergeRegion = String(this.index)
        const actions: RegionAction[] = this.kind === "conflict"
            ? ["ours", "theirs", "both", "ignore"]
            : this.kind === "both" ? ["ours", "ignore"] : [this.kind, "ignore"]
        for (const action of actions) {
            const button = document.createElement("button")
            button.type = "button"
            button.textContent = this.labels[action]
            button.addEventListener("mousedown", (event) => event.preventDefault())
            button.addEventListener("click", () => this.act(this.index, action))
            wrap.append(button)
        }
        return wrap
    }
    ignoreEvent() {
        return true
    }
}

function resultExtension(
    regions: readonly MergeRegion[],
    initial: ResultRegion[],
    labels: Record<RegionAction, string>,
    act: (index: number, action: RegionAction) => void
): { field: StateField<ResultRegion[]>; extension: Extension } {
    const field = StateField.define<ResultRegion[]>({
        create: () => initial,
        update(value, tr) {
            let next = tr.docChanged
                ? value.map((region) => ({
                    ...region,
                    from: tr.changes.mapPos(region.from, -1),
                    to: tr.changes.mapPos(region.to, 1)
                }))
                : value
            for (const effect of tr.effects) {
                if (!effect.is(setRegion)) continue
                const { index, ...region } = effect.value
                next = next.map((current, i) => i === index ? region : current)
            }
            return next
        }
    })
    const decorations = EditorView.decorations.compute([field], (state) => {
        const current = state.field(field)
        const lines = lineDecorations(state, current.map((region, i) => ({
            from: region.from,
            to: region.to,
            className: region.resolved ? "cm-merge-resolved" : regionClass[regions[i].kind]
        })))
        const builder = new RangeSetBuilder<Decoration>()
        const widgets = current
            .map((region, index) => ({ region, index }))
            .filter(({ region }) => !region.resolved)
            .sort((a, b) => a.region.from - b.region.from)
        for (const { region, index } of widgets) {
            const at = state.doc.lineAt(Math.min(region.from, state.doc.length)).from
            builder.add(at, at, Decoration.widget({
                widget: new RegionActionsWidget(index, regions[index].kind, labels, act),
                block: true,
                side: -1
            }))
        }
        return RangeSet.join([lines, builder.finish()])
    })
    return { field, extension: [field, decorations] }
}

function baseExtensions(path: string, content: string): Extension[] {
    return [
        appTheme,
        syntaxHighlighting(appHighlightStyle),
        lineNumbers(),
        mergeTheme,
        ...languageExtensions(path, hasVeryLongLine(content))
    ]
}

function MergeEditors({ path, texts, onDone }: { path: string; texts: MergeTexts; onDone: () => void }) {
    const { t } = useTranslation("menus")
    const runOp = useGitStore((s) => s.runOp)
    const repositoryRoot = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const busy = useGitStore((s) => s.busy)
    const leftRef = useRef<HTMLDivElement>(null)
    const centerRef = useRef<HTMLDivElement>(null)
    const rightRef = useRef<HTMLDivElement>(null)
    const viewsRef = useRef<{ ours: EditorView; result: EditorView; theirs: EditorView } | null>(null)
    const fieldRef = useRef<StateField<ResultRegion[]> | null>(null)
    // Null until the editors exist, so no action runs against missing views.
    const [resolved, setResolved] = useState<boolean[] | null>(null)

    const model = useMemo(() => {
        const base = splitLines(texts.base)
        const ours = splitLines(texts.ours)
        const theirs = splitLines(texts.theirs)
        return {
            base,
            ours,
            theirs,
            regions: mergeRegions(base, ours, theirs),
            baseOffsets: offsets(base),
            oursOffsets: offsets(ours),
            theirsOffsets: offsets(theirs)
        }
    }, [texts])

    const sliceOf = (side: "base" | "ours" | "theirs", region: MergeRegion) =>
        model[side].slice(region[side][0], region[side][1]).join("")

    const apply = (index: number, action: RegionAction) => {
        const views = viewsRef.current
        const field = fieldRef.current
        if (!views || !field) return
        const current = views.result.state.field(field)[index]
        const region = model.regions[index]
        if (!current || current.resolved) return
        const insert = action === "ours" ? sliceOf("ours", region)
            : action === "theirs" ? sliceOf("theirs", region)
                : action === "both" ? sliceOf("ours", region) + sliceOf("theirs", region)
                    : null
        views.result.dispatch({
            changes: insert === null ? undefined : { from: current.from, to: current.to, insert },
            effects: setRegion.of({
                index,
                from: current.from,
                to: insert === null ? current.to : current.from + insert.length,
                resolved: true
            })
        })
    }
    const applyRef = useRef(apply)
    useEffect(() => {
        applyRef.current = apply
    })

    useEffect(() => {
        const left = leftRef.current
        const center = centerRef.current
        const right = rightRef.current
        if (!left || !center || !right) return
        const labels: Record<RegionAction, string> = {
            ours: t("gitMerge.acceptYours"),
            theirs: t("gitMerge.acceptTheirs"),
            both: t("gitMerge.acceptBoth"),
            ignore: t("gitMerge.ignore")
        }
        const initial = model.regions.map((region) => ({
            from: model.baseOffsets[region.base[0]],
            to: model.baseOffsets[region.base[1]],
            resolved: false
        }))
        const result = resultExtension(model.regions, initial, labels, (index, action) => applyRef.current(index, action))
        fieldRef.current = result.field
        const ours = new EditorView({
            parent: left,
            state: EditorState.create({
                doc: texts.ours,
                extensions: [...baseExtensions(path, texts.ours), EditorState.readOnly.of(true), EditorView.editable.of(false), sideExtension(model.regions, "ours", model.oursOffsets)]
            })
        })
        const theirs = new EditorView({
            parent: right,
            state: EditorState.create({
                doc: texts.theirs,
                extensions: [...baseExtensions(path, texts.theirs), EditorState.readOnly.of(true), EditorView.editable.of(false), sideExtension(model.regions, "theirs", model.theirsOffsets)]
            })
        })
        const resultView = new EditorView({
            parent: center,
            state: EditorState.create({
                doc: texts.base,
                extensions: [
                    ...baseExtensions(path, texts.base),
                    result.extension,
                    EditorView.updateListener.of((update) => {
                        const before = update.startState.field(result.field)
                        const after = update.state.field(result.field)
                        if (before !== after) setResolved(after.map((region) => region.resolved))
                    })
                ]
            })
        })
        viewsRef.current = { ours, result: resultView, theirs }
        setResolved(initial.map(() => false))
        return () => {
            ours.destroy()
            theirs.destroy()
            resultView.destroy()
            viewsRef.current = null
            fieldRef.current = null
            setResolved(null)
        }
    }, [model, path, texts, t])

    const pending = resolved ? model.regions.map((_, index) => index).filter((index) => !resolved[index]) : []
    const pendingConflicts = pending.filter((index) => model.regions[index].kind === "conflict")

    const reveal = (index: number) => {
        const views = viewsRef.current
        const field = fieldRef.current
        if (!views || !field) return
        const region = model.regions[index]
        const current = views.result.state.field(field)[index]
        views.result.dispatch({ effects: EditorView.scrollIntoView(current.from, { y: "center" }), selection: { anchor: current.from } })
        views.ours.dispatch({ effects: EditorView.scrollIntoView(model.oursOffsets[region.ours[0]], { y: "center" }) })
        views.theirs.dispatch({ effects: EditorView.scrollIntoView(model.theirsOffsets[region.theirs[0]], { y: "center" }) })
    }
    const step = (direction: 1 | -1) => {
        const views = viewsRef.current
        const field = fieldRef.current
        if (!views || !field || !pending.length) return
        const cursor = views.result.state.selection.main.head
        const regions = views.result.state.field(field)
        const ordered = [...pending].sort((a, b) => regions[a].from - regions[b].from)
        const next = direction === 1
            ? ordered.find((index) => regions[index].from > cursor) ?? ordered[0]
            : [...ordered].reverse().find((index) => regions[index].from < cursor) ?? ordered[ordered.length - 1]
        reveal(next)
    }

    const applyNonConflicting = (scope: "ours" | "theirs" | "all") => {
        for (const index of pending) {
            const kind = model.regions[index].kind
            if (kind === "conflict") continue
            if (kind === "both") apply(index, "ours")
            else if (scope === "all" || scope === kind) apply(index, kind)
        }
    }

    const resolveSimple = () => {
        const views = viewsRef.current
        const field = fieldRef.current
        if (!views || !field) return
        for (const index of pendingConflicts) {
            const region = model.regions[index]
            const merged = resolveSimpleConflict(sliceOf("base", region), sliceOf("ours", region), sliceOf("theirs", region))
            if (merged === null) continue
            const current = views.result.state.field(field)[index]
            views.result.dispatch({
                changes: { from: current.from, to: current.to, insert: merged },
                effects: setRegion.of({ index, from: current.from, to: current.from + merged.length, resolved: true })
            })
        }
    }

    const save = async () => {
        const views = viewsRef.current
        if (!views || !repositoryRoot || busy != null) return
        if (pending.length) {
            const ok = await requestAppConfirmation({
                title: t("gitMerge.applyTitle"),
                description: t("gitMerge.applyUnresolved", { count: pending.length }),
                kind: "warning"
            })
            if (!ok) return
        }
        let content = views.result.state.doc.toString()
        if (texts.crlf) content = content.replace(/\r?\n/g, "\r\n")
        const root = repositoryRoot
        const absolute = nativePathJoin(root, isWindowsPath(root) ? path.replace(/\//g, "\\") : path)
        // The result is built from the merge base, so it replaces any manual
        // work on the file: unsaved editor edits, or conflicts resolved by hand.
        const unsaved = dirtyTabPaths().includes(absolute)
        const worktree = await gitConflictSides(root, path)
            .then((sides) => readableText(sides.worktree))
            .catch(() => texts.worktree)
        const conflicts = model.regions.filter((region) => region.kind === "conflict").length
        const edited = worktree !== texts.worktree || (worktree !== null && conflictMarkerCount(worktree) < conflicts)
        if (unsaved || edited) {
            const replace = await requestAppConfirmation({
                title: t("gitMerge.applyTitle"),
                description: t(unsaved ? "gitMerge.replaceUnsaved" : "gitMerge.replaceManual"),
                kind: "warning"
            })
            if (!replace) return
        }
        const ok = await runOp("conflict-merge", async () => {
            await saveFile(absolute, content)
            await gitStage(root, [path])
        })
        if (ok) {
            void logUserAction("git_conflict_merge", `merge ${path}`)
            onDone()
        }
    }

    return (
        <>
            <div className="flex flex-wrap items-center gap-[6px]">
                <Button type="button" size="xs" variant="outline" aria-label={t("gitMerge.previous")} title={t("gitMerge.previous")} disabled={!pending.length} onClick={() => step(-1)}>
                    <ChevronUp aria-hidden="true" />
                </Button>
                <Button type="button" size="xs" variant="outline" aria-label={t("gitMerge.next")} title={t("gitMerge.next")} disabled={!pending.length} onClick={() => step(1)}>
                    <ChevronDown aria-hidden="true" />
                </Button>
                <span role="status" className="text-[11.5px] text-(--ink-2)">
                    {resolved ? t("gitMerge.remaining", { changes: pending.length, conflicts: pendingConflicts.length }) : t("gitMerge.loading")}
                </span>
                <span className="flex-1" />
                <Button type="button" size="xs" variant="outline" disabled={!pending.length} onClick={() => applyNonConflicting("ours")}>{t("gitMerge.applyLeft")}</Button>
                <Button type="button" size="xs" variant="outline" disabled={!pending.length} onClick={() => applyNonConflicting("all")}>{t("gitMerge.applyNonConflicting")}</Button>
                <Button type="button" size="xs" variant="outline" disabled={!pending.length} onClick={() => applyNonConflicting("theirs")}>{t("gitMerge.applyRight")}</Button>
                <Button type="button" size="xs" variant="outline" disabled={!pendingConflicts.length} onClick={resolveSimple}>
                    <Wand2 data-icon="inline-start" aria-hidden="true" />{t("gitMerge.resolveSimple")}
                </Button>
            </div>
            <div className="grid min-h-0 flex-1 grid-cols-3 gap-[8px]">
                {([
                    ["yours", leftRef],
                    ["result", centerRef],
                    ["theirs", rightRef]
                ] as const).map(([pane, ref]) => (
                    <section key={pane} aria-label={t(`gitMerge.pane.${pane}`)} className="flex min-h-0 flex-col overflow-hidden rounded-[8px] border border-(--line-1)">
                        <header className="shrink-0 border-b border-(--line-1) bg-(--paper-1) px-[8px] py-[4px] text-[11px] font-semibold text-(--ink-2)">
                            {t(`gitMerge.pane.${pane}`)}
                        </header>
                        <div ref={ref} data-merge-pane={pane} className="min-h-0 flex-1 overflow-auto [&_.cm-editor]:h-full" />
                    </section>
                ))}
            </div>
            <DialogFooter>
                <Button type="button" variant="ghost" onClick={onDone}>{t("gitMerge.cancel")}</Button>
                <Button type="button" disabled={busy != null || !repositoryRoot || !resolved} onClick={() => void save()}>{t("gitMerge.apply")}</Button>
            </DialogFooter>
        </>
    )
}

/** Three-pane merge tool (Yours | Result | Theirs) for one conflicted file. */
export function GitMergeTool() {
    const { t } = useTranslation("menus")
    const path = useGitConflictStore((s) => s.mergePath)
    const close = useGitConflictStore((s) => s.closeMerge)
    const repositoryRoot = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const [loaded, setLoaded] = useState<{ path: string; texts?: MergeTexts; error?: string } | null>(null)
    // A result for another path (or none yet) means this path is still loading.
    const state = loaded?.path === path ? loaded : null
    useOverlayPresence(path !== null)

    useEffect(() => {
        if (!path || !repositoryRoot) return
        let cancelled = false
        const setState = (next: { path: string; texts?: MergeTexts; error?: string }) => { if (!cancelled) setLoaded(next) }
        gitConflictSides(repositoryRoot, path).then((sides) => {
            // The left pane is the user's side, which a rebase stores as theirs.
            const rebase = useGitStore.getState().status?.inProgress === "rebase"
            const texts = mergeTexts(rebase ? { ...sides, ours: sides.theirs, theirs: sides.ours } : sides)
            setState(typeof texts === "string"
                ? { path, error: t(texts === "deleted" ? "gitMerge.deletedSide" : "gitMerge.binary") }
                : { path, texts })
        }).catch((error: unknown) => {
            setState({ path, error: String(error) })
        })
        return () => { cancelled = true }
    }, [path, repositoryRoot, t])

    return (
        <Dialog open={path !== null} onOpenChange={(next) => { if (!next) close() }}>
            {path && (
                <DialogContent
                    resizeId="git-merge"
                    minSize={dialogMinSize(760, 420)}
                    className="flex min-h-0 flex-col gap-[10px]"
                    onEscapeKeyDown={(event) => event.preventDefault()}
                >
                    <DialogHeader>
                        <DialogTitle>{t("gitMerge.title", { path })}</DialogTitle>
                        <DialogDescription>{t("gitMerge.description")}</DialogDescription>
                    </DialogHeader>
                    {state?.texts ? (
                        <MergeEditors key={state.path} path={state.path} texts={state.texts} onDone={close} />
                    ) : (
                        <div role={state?.error ? "alert" : "status"} className="flex min-h-0 flex-1 items-center justify-center text-[12px] text-(--ink-3)">
                            {state?.error ?? t("gitMerge.loading")}
                        </div>
                    )}
                </DialogContent>
            )}
        </Dialog>
    )
}
