import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import * as ipc from "../lib/ipc"
import { useWorkspaceStore } from "../state/workspaceStore"
import { clearAll, documentGeneration } from "./documentRegistry"
import { getView, registerView, subscribeView, unregisterView } from "./viewRegistry"
import { saveDirtyTab } from "./saveDocument"

vi.mock("../lib/ipc", () => ({ saveFile: vi.fn(async () => 0), openFileSnapshot: vi.fn() }))
vi.mock("../lib/actionFeedback", () => ({ showActionError: vi.fn(async () => {}) }))
const initial = useWorkspaceStore.getState()
const mounted: { path: string; view: EditorView }[] = []
const subscriptions: (() => void)[] = []
function mount(path: string, content: string) {
    const view = new EditorView({ state: EditorState.create({ doc: content }), parent: document.body })
    mounted.push({ path, view })
    registerView(path, view)
    const store = useWorkspaceStore.getState()
    store.openTab(path)
    store.hydrateLineEnding(path, "lf", documentGeneration(path))
    store.markDirty(path, true)
    return view
}
beforeEach(() => {
    clearAll()
    useWorkspaceStore.setState({ ...initial, workspacePath: "/repo" }, true)
    vi.mocked(ipc.saveFile).mockClear()
})
afterEach(() => {
    for (const dispose of subscriptions.splice(0)) dispose()
    for (const { path, view } of mounted.splice(0)) { unregisterView(path, view); view.destroy() }
    clearAll()
})

it.each([
    ["/repo/\\a.txt", "/repo/a.txt", false],
    ["/repo/\\a.txt", "/repo/a.txt", true],
    ["/repo/a\\b.txt", "/repo/a/b.txt", false],
    ["/repo/a\\b.txt", "/repo/a/b.txt", true],
] as const)("saves distinct POSIX paths %s and %s (reverse=%s) from their own live buffers", async (literal, ordinary, reverse) => {
    const paths = reverse ? [ordinary, literal] : [literal, ordinary]
    for (const path of paths) mount(path, path === literal ? "literal buffer" : "ordinary buffer")
    await expect(saveDirtyTab(literal)).resolves.toEqual({ kind: "saved" })
    expect(ipc.saveFile).toHaveBeenNthCalledWith(1, literal, "literal buffer")
    const tabs = useWorkspaceStore.getState().groups[0].tabs
    expect(tabs.find(tab => tab.path === literal)?.dirty).toBe(false)
    expect(tabs.find(tab => tab.path === ordinary)?.dirty).toBe(true)
    await expect(saveDirtyTab(ordinary)).resolves.toEqual({ kind: "saved" })
    expect(ipc.saveFile).toHaveBeenNthCalledWith(2, ordinary, "ordinary buffer")
})

it.each([false, true])("keeps view subscriptions and unregister independent for POSIX literals (reverse=%s)", async reverse => {
    const literal = "/repo/\\a.txt", ordinary = "/repo/a.txt"
    const paths = reverse ? [ordinary, literal] : [literal, ordinary]
    const views = new Map(paths.map(path => [path, mount(path, path)]))
    const literalChanged = vi.fn(), ordinaryChanged = vi.fn()
    subscriptions.push(subscribeView(literal, literalChanged), subscribeView(ordinary, ordinaryChanged))
    views.get(literal)!.dispatch({ changes: { from: 0, insert: "edited " } })
    await Promise.resolve()
    expect(literalChanged).toHaveBeenCalledExactlyOnceWith("document")
    expect(ordinaryChanged).not.toHaveBeenCalled()
    unregisterView(literal, views.get(literal))
    await Promise.resolve()
    expect(getView(literal)).toBeUndefined()
    expect(getView(ordinary)).toBe(views.get(ordinary))
    expect(ordinaryChanged).not.toHaveBeenCalled()
})

it("keeps exact tab labels and closes only the selected literal path", () => {
    const literal = "/repo/\\a.txt", ordinary = "/repo/a.txt"
    mount(literal, "literal"); mount(ordinary, "ordinary")
    expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => [tab.path, tab.name])).toEqual([
        [literal, "\\a.txt"], [ordinary, "a.txt"],
    ])
    useWorkspaceStore.getState().closeTab(0, literal)
    expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual([ordinary])
    expect(getView(ordinary)?.state.doc.toString()).toBe("ordinary")
})
