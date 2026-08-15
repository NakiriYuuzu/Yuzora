import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const documentMock = vi.hoisted(() => ({
    getDocument: vi.fn()
}))
const ipcMock = vi.hoisted(() => ({
    isOpenableFile: vi.fn()
}))
const feedbackMock = vi.hoisted(() => ({
    showActionError: vi.fn(async (_action: string, _error: unknown) => undefined)
}))

vi.mock("../editor/documentRegistry", () => ({
    getDocument: (path: string) => documentMock.getDocument(path)
}))
vi.mock("../lib/ipc", () => ({
    isOpenableFile: (path: string) => ipcMock.isOpenableFile(path)
}))
vi.mock("../lib/actionFeedback", () => ({
    showActionError: (action: string, error: unknown) => feedbackMock.showActionError(action, error)
}))

import { Terminal } from "@xterm/xterm"

import type { HerdrSnapshot } from "../lib/herdrTypes"
import { previewInitialState, usePreviewStore } from "../state/previewStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import {
    classifyTerminalTargetToken,
    createTerminalTargetContextGate,
    createTerminalTargetLinkProvider,
    installTerminalTargetOpen,
    findTerminalTargetsInText,
    isTerminalTargetOpenGesture,
    openTerminalTarget,
    resolveHerdrTerminalBaseCwd,
    type TerminalTarget,
    type TerminalTargetBufferHost,
    type TerminalTargetLink
} from "./terminalTarget"

const initialWorkspace = useWorkspaceStore.getState()

function mouse(
    partial: Partial<Pick<MouseEvent, "button" | "metaKey" | "ctrlKey" | "altKey">> = {}
): Pick<MouseEvent, "button" | "metaKey" | "ctrlKey" | "altKey"> {
    return {
        button: 0,
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        ...partial
    }
}

function fakeHost(
    lines: Array<{ text: string; wrapped?: boolean }>,
    cols = 80
): TerminalTargetBufferHost {
    return {
        cols,
        buffer: {
            active: {
                length: lines.length,
                getLine(y: number) {
                    const line = lines[y]
                    if (!line) return undefined
                    return {
                        isWrapped: Boolean(line.wrapped),
                        length: line.text.length,
                        getCell(x: number) {
                            const char = line.text[x]
                            if (char === undefined) return undefined
                            return {
                                getChars: () => char,
                                getWidth: () => 1
                            }
                        },
                        translateToString: (trimRight?: boolean) =>
                            trimRight ? line.text.replace(/\s+$/, "") : line.text
                    }
                }
            }
        }
    }
}

async function provide(
    host: TerminalTargetBufferHost,
    line: number,
    cwd: string | null,
    onActivate = vi.fn()
) {
    const provider = createTerminalTargetLinkProvider(host, {
        getCwd: () => cwd,
        onActivate,
        isMac: () => true
    })
    const links = await new Promise<TerminalTargetLink[] | undefined>((resolve) => {
        provider.provideLinks(line, resolve)
    })
    return { links, onActivate, provider }
}

describe("findTerminalTargetsInText", () => {
    it("gives http/https precedence over a file-like suffix", () => {
        const matches = findTerminalTargetsInText("https://example.com/src/app.ts:12", "/ws")
        expect(matches).toHaveLength(1)
        expect(matches[0]?.target).toEqual({
            kind: "url",
            url: "https://example.com/src/app.ts:12"
        })
    })

    it("allowlists only http and https for browser navigation", () => {
        expect(classifyTerminalTargetToken("http://localhost:3000/docs", null)).toEqual({
            kind: "url",
            url: "http://localhost:3000/docs"
        })
        expect(classifyTerminalTargetToken("https://example.com/", null)).toEqual({
            kind: "url",
            url: "https://example.com/"
        })
    })

    it("parses POSIX absolute, relative, quoted, spaced, and file URL paths", () => {
        expect(findTerminalTargetsInText("/tmp/app.ts", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts"
        })
        expect(findTerminalTargetsInText("src/app.ts", "/ws")[0]?.target).toEqual({
            kind: "file",
            path: "/ws/src/app.ts"
        })
        expect(findTerminalTargetsInText("\"./my file.ts\"", "/ws")[0]?.target).toEqual({
            kind: "file",
            path: "/ws/my file.ts"
        })
        expect(findTerminalTargetsInText("'/tmp/spaced name.ts'", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/spaced name.ts"
        })
        expect(findTerminalTargetsInText("file:///tmp/from-url.ts", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/from-url.ts"
        })
    })

    it("strips common trailing punctuation and wrappers without a general shell parser", () => {
        expect(findTerminalTargetsInText("see /tmp/app.ts.", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts"
        })
        expect(findTerminalTargetsInText("(/tmp/app.ts)", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts"
        })
        expect(findTerminalTargetsInText("https://example.com/a).", null)[0]?.target).toEqual({
            kind: "url",
            url: "https://example.com/a"
        })
    })

    it("parses path:line[:column] and keeps a validated 1-based line", () => {
        expect(findTerminalTargetsInText("/tmp/app.ts:12", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts",
            line: 12
        })
        expect(findTerminalTargetsInText("/tmp/app.ts:12:4", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts",
            line: 12,
            column: 4
        })
        expect(findTerminalTargetsInText("/tmp/app.ts:0", null)[0]?.target).toEqual({
            kind: "file",
            path: "/tmp/app.ts:0"
        })
    })

    it("preserves Windows drive, UNC, and verbatim paths without rewriting them", () => {
        expect(findTerminalTargetsInText(String.raw`C:\Users\me\app.ts:8`, null)[0]?.target).toEqual({
            kind: "file",
            path: String.raw`C:\Users\me\app.ts`,
            line: 8
        })
        expect(findTerminalTargetsInText(String.raw`\\server\share\app.ts`, null)[0]?.target).toEqual({
            kind: "file",
            path: String.raw`\\server\share\app.ts`
        })
        expect(findTerminalTargetsInText(String.raw`\\?\C:\Users\me\app.ts`, null)[0]?.target).toEqual({
            kind: "file",
            path: String.raw`\\?\C:\Users\me\app.ts`
        })
        expect(findTerminalTargetsInText("src\\app.ts", String.raw`\\?\C:\proj`)[0]?.target).toEqual({
            kind: "file",
            path: String.raw`\\?\C:\proj\src\app.ts`
        })
    })

    it("rejects malformed, control, custom, and remote schemes", () => {
        expect(classifyTerminalTargetToken("javascript:alert(1)", null)).toBeNull()
        expect(classifyTerminalTargetToken("data:text/html,hi", null)).toBeNull()
        expect(classifyTerminalTargetToken("ssh://host/tmp/a.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("vscode://file/tmp/a.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("sftp://host/tmp/a.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("yuzora://preview", null)).toBeNull()
        expect(classifyTerminalTargetToken("x:notes.txt", "/ws")).toBeNull()
        expect(classifyTerminalTargetToken("file:///tmp/%zz.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("/tmp/bad\u0000.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("src/app.ts", null)).toBeNull()
        expect(classifyTerminalTargetToken("done", "/ws")).toBeNull()
    })
})

describe("createTerminalTargetLinkProvider", () => {
    beforeEach(() => {
        documentMock.getDocument.mockReset()
        documentMock.getDocument.mockResolvedValue({
            result: { kind: "full", content: "ok", size: 2, lineEnding: "lf" }
        })
        ipcMock.isOpenableFile.mockReset()
        ipcMock.isOpenableFile.mockResolvedValue(true)
    })

    it("maps public buffer ranges onto exact tokens", async () => {
        const { links } = await provide(
            fakeHost([{ text: "see /tmp/app.ts and more" }]),
            1,
            null
        )
        await vi.waitFor(() => expect(links).toHaveLength(1))
        expect(links?.[0]).toMatchObject({
            text: "/tmp/app.ts",
            range: {
                start: { x: 5, y: 1 },
                end: { x: 15, y: 1 }
            }
        })
        expect(links?.[0]?.decorations).toEqual({
            pointerCursor: false,
            underline: true
        })

        const hover = links?.[0]?.hover
        const leave = links?.[0]?.leave
        const link = links?.[0]
        hover?.({ metaKey: false, ctrlKey: false, altKey: false } as MouseEvent, link?.text ?? "")
        expect(link?.decorations).toEqual({ pointerCursor: false, underline: true })
        hover?.({ metaKey: true, ctrlKey: false, altKey: false } as MouseEvent, link?.text ?? "")
        expect(link?.decorations).toEqual({ pointerCursor: true, underline: true })
        leave?.({} as MouseEvent, link?.text ?? "")
        expect(link?.decorations).toEqual({ pointerCursor: false, underline: true })
    })

    it("maps wide, combining, and wrapped xterm cells instead of UTF-16 offsets", async () => {
        async function linksFor(text: string) {
            const terminal = new Terminal({ cols: 10, rows: 5 })
            await new Promise<void>((resolve) => terminal.write(text, resolve))
            const result = await provide(terminal, 1, null)
            terminal.dispose()
            return result.links
        }

        expect((await linksFor("漢 /tmp/a.ts"))?.[0]?.range).toEqual({
            start: { x: 4, y: 1 },
            end: { x: 2, y: 2 }
        })
        expect((await linksFor("e\u0301 /tmp/a.ts"))?.[0]?.range).toEqual({
            start: { x: 3, y: 1 },
            end: { x: 1, y: 2 }
        })
        expect((await linksFor("😀 /tmp/a.ts"))?.[0]?.range).toEqual({
            start: { x: 3, y: 1 },
            end: { x: 1, y: 2 }
        })
        const wideWrap = await linksFor("/tmp/aaaa漢.ts")
        expect(wideWrap?.[0]).toMatchObject({
            text: "/tmp/aaaa漢.ts",
            range: {
                start: { x: 1, y: 1 },
                end: { x: 5, y: 2 }
            }
        })
    })

    it("omits unreadable file targets before xterm can decorate them", async () => {
        ipcMock.isOpenableFile.mockResolvedValueOnce(false)
        const provider = createTerminalTargetLinkProvider(
            fakeHost([{ text: "missing.ts  https://example.com/docs" }]),
            {
                getCwd: () => "/Users/ws",
                onActivate: vi.fn(),
                isMac: () => true
            }
        )

        const links = await new Promise<TerminalTargetLink[] | undefined>((resolve) => {
            provider.provideLinks(1, resolve)
        })

        expect(ipcMock.isOpenableFile).toHaveBeenCalledWith("/Users/ws/missing.ts")
        expect(links?.map((link) => link.text)).toEqual(["https://example.com/docs"])
        expect(links?.[0]?.decorations).toEqual({
            pointerCursor: false,
            underline: true
        })
    })

    it("drops stale async file validation after terminal output changes", async () => {
        let finishValidation!: () => void
        ipcMock.isOpenableFile.mockImplementationOnce(
            () => new Promise((resolve) => {
                finishValidation = () => resolve(true)
            })
        )
        const first = { text: "old.ts" }
        const host = fakeHost([first])
        const provider = createTerminalTargetLinkProvider(host, {
            getCwd: () => "/Users/ws",
            onActivate: vi.fn(),
            isMac: () => true
        })
        const pending = new Promise<TerminalTargetLink[] | undefined>((resolve) => {
            provider.provideLinks(1, resolve)
        })

        first.text = "new.ts"
        finishValidation()

        await expect(pending).resolves.toBeUndefined()
    })

    it("bounds and refreshes file checks instead of caching stale misses forever", async () => {
        vi.useFakeTimers()
        ipcMock.isOpenableFile
            .mockResolvedValueOnce(false)
            .mockResolvedValueOnce(true)
        const provider = createTerminalTargetLinkProvider(
            fakeHost([{ text: "later.ts" }]),
            {
                getCwd: () => "/Users/ws",
                onActivate: vi.fn(),
                isMac: () => true
            }
        )
        const provideOnce = () => new Promise<TerminalTargetLink[] | undefined>((resolve) => {
            provider.provideLinks(1, resolve)
        })

        await expect(provideOnce()).resolves.toBeUndefined()
        await expect(provideOnce()).resolves.toBeUndefined()
        expect(ipcMock.isOpenableFile).toHaveBeenCalledTimes(1)

        vi.advanceTimersByTime(2_001)
        await expect(provideOnce()).resolves.toHaveLength(1)
        expect(ipcMock.isOpenableFile).toHaveBeenCalledTimes(2)
        vi.useRealTimers()
    })

    it("joins wrapped public buffer lines and stays safe on blank lines", async () => {
        const wrapped = await provide(
            fakeHost([
                { text: "https://ex" },
                { text: ".com/a", wrapped: true }
            ], 10),
            2,
            null
        )
        await vi.waitFor(() => expect(wrapped.links).toHaveLength(1))
        expect(wrapped.links?.[0]?.text).toBe("https://ex.com/a")
        expect(wrapped.links?.[0]?.range).toEqual({
            start: { x: 1, y: 1 },
            end: { x: 6, y: 2 }
        })

        const blank = await provide(fakeHost([{ text: "   " }, { text: "" }]), 2, "/ws")
        expect(blank.links).toBeUndefined()

        const missing = await provide(fakeHost([{ text: "/tmp/a.ts" }]), 4, null)
        expect(missing.links).toBeUndefined()
    })
})

describe("terminal target mouse gestures", () => {
    it("accepts Mac Cmd-left and rejects plain, Ctrl, Alt, and right click", () => {
        expect(isTerminalTargetOpenGesture(mouse({ metaKey: true }), { mac: true })).toBe(true)
        expect(isTerminalTargetOpenGesture(mouse({}), { mac: true })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ ctrlKey: true }), { mac: true })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ metaKey: true, ctrlKey: true }), { mac: true })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ metaKey: true, altKey: true }), { mac: true })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ button: 2, metaKey: true }), { mac: true })).toBe(false)
    })

    it("accepts Windows/Linux Ctrl-left and rejects plain, Meta, and right click", () => {
        expect(isTerminalTargetOpenGesture(mouse({ ctrlKey: true }), { mac: false })).toBe(true)
        expect(isTerminalTargetOpenGesture(mouse({}), { mac: false })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ metaKey: true }), { mac: false })).toBe(false)
        expect(isTerminalTargetOpenGesture(mouse({ button: 2, ctrlKey: true }), { mac: false })).toBe(false)
    })

    it("opens a recognized target on Cmd-left and ignores plain/right/middle", async () => {
        const { links, onActivate } = await provide(
            fakeHost([{ text: "/tmp/app.ts" }]),
            1,
            null
        )
        links?.[0]?.activate(mouse({ button: 2, metaKey: true }) as MouseEvent, "/tmp/app.ts")
        links?.[0]?.activate(mouse({ button: 1, metaKey: true }) as MouseEvent, "/tmp/app.ts")
        links?.[0]?.activate(mouse({ button: 0 }) as MouseEvent, "/tmp/app.ts")
        expect(onActivate).not.toHaveBeenCalled()
        links?.[0]?.activate(mouse({ button: 0, metaKey: true }) as MouseEvent, "/tmp/app.ts")
        expect(onActivate).toHaveBeenCalledTimes(1)
    })
})

describe("installTerminalTargetOpen", () => {
    const workspaceSnapshot = useWorkspaceStore.getState()

    beforeEach(() => {
        useWorkspaceStore.setState({
            ...workspaceSnapshot,
            workspacePath: "/ws",
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0
        })
        usePreviewStore.setState({ ...previewInitialState })
    })

    afterEach(() => {
        useWorkspaceStore.setState(workspaceSnapshot, true)
    })

    it("routes OSC 8 Cmd-left to Preview and keeps plain/right click inert", () => {
        const linkProviderDisposable = { dispose: vi.fn() }
        const terminal = {
            cols: 80,
            rows: 24,
            buffer: fakeHost([]).buffer,
            element: document.createElement("div"),
            options: {},
            refresh: vi.fn(),
            registerLinkProvider: vi.fn(() => linkProviderDisposable)
        } as unknown as Terminal
        const installed = installTerminalTargetOpen(terminal, {
            getCwd: () => "/ws",
            isMac: () => true
        })
        const handler = terminal.options.linkHandler
        expect(handler).toBeTruthy()
        const openSpy = vi.spyOn(window, "open")

        handler?.activate(
            mouse({ button: 2, metaKey: true }) as MouseEvent,
            "https://example.com/right",
            { start: { x: 1, y: 1 }, end: { x: 5, y: 1 } }
        )
        expect(usePreviewStore.getState().navForWorkspace("/ws").url).not.toBe(
            "https://example.com/right"
        )

        handler?.activate(
            mouse({ button: 0 }) as MouseEvent,
            "https://example.com/plain",
            { start: { x: 1, y: 1 }, end: { x: 5, y: 1 } }
        )
        expect(usePreviewStore.getState().navForWorkspace("/ws").url).not.toBe(
            "https://example.com/plain"
        )

        handler?.activate(
            mouse({ button: 0, metaKey: true }) as MouseEvent,
            "https://example.com/left",
            { start: { x: 1, y: 1 }, end: { x: 5, y: 1 } }
        )
        expect(usePreviewStore.getState().navForWorkspace("/ws").url).toBe(
            "https://example.com/left"
        )
        expect(useWorkspaceStore.getState().groups[0]?.activePath).toBe(PREVIEW_TAB_PATH)
        expect(openSpy).not.toHaveBeenCalled()

        installed.resetHover()
        expect(terminal.refresh).toHaveBeenCalledWith(0, terminal.rows - 1)

        installed.dispose()
        openSpy.mockRestore()
        expect(linkProviderDisposable.dispose).toHaveBeenCalledTimes(1)
        expect(terminal.options.linkHandler).toBeNull()
    })
})

describe("openTerminalTarget", () => {
    beforeEach(() => {
        documentMock.getDocument.mockReset()
        documentMock.getDocument.mockResolvedValue({
            result: { kind: "full", content: "ok", size: 2, lineEnding: "lf" }
        })
        feedbackMock.showActionError.mockClear()
        useWorkspaceStore.setState({
            ...initialWorkspace,
            workspacePath: "/ws",
            groups: [
                { tabs: [], activePath: null },
                { tabs: [], activePath: null }
            ],
            activeGroupIndex: 1,
            pendingReveal: null
        })
        usePreviewStore.setState({ ...previewInitialState })
    })

    afterEach(() => {
        useWorkspaceStore.setState(initialWorkspace, true)
    })

    it("opens a successful file in the captured active group after getDocument", async () => {
        await openTerminalTarget(
            { kind: "file", path: "/ext/abs.ts" },
            { triggerGroupIndex: 1 }
        )
        expect(documentMock.getDocument).toHaveBeenCalledWith("/ext/abs.ts")
        expect(useWorkspaceStore.getState().groups[1]?.activePath).toBe("/ext/abs.ts")
        expect(useWorkspaceStore.getState().groups[0]?.tabs).toEqual([])
        expect(feedbackMock.showActionError).not.toHaveBeenCalled()
    })

    it("moves an existing file tab into the captured terminal group", async () => {
        useWorkspaceStore.getState().openTab("/ext/abs.ts", 0)
        useWorkspaceStore.getState().markDirty("/ext/abs.ts", true)

        await openTerminalTarget(
            { kind: "file", path: "/ext/abs.ts", line: 7 },
            { triggerGroupIndex: 1 }
        )

        expect(useWorkspaceStore.getState().groups[0]?.tabs).toEqual([])
        expect(useWorkspaceStore.getState().groups[1]?.activePath).toBe("/ext/abs.ts")
        expect(useWorkspaceStore.getState().groups[1]?.tabs[0]).toMatchObject({
            path: "/ext/abs.ts",
            dirty: true
        })
        expect(useWorkspaceStore.getState().activeGroupIndex).toBe(1)
        expect(useWorkspaceStore.getState().pendingReveal).toEqual({
            path: "/ext/abs.ts",
            line: 7
        })
    })

    it("reports an app dialog and opens no tab when getDocument fails", async () => {
        documentMock.getDocument.mockRejectedValue(new Error("missing"))
        await openTerminalTarget(
            { kind: "file", path: "/missing.ts" },
            { triggerGroupIndex: 1 }
        )
        expect(useWorkspaceStore.getState().groups[1]?.tabs).toEqual([])
        expect(feedbackMock.showActionError).toHaveBeenCalledTimes(1)
        expect(feedbackMock.showActionError.mock.calls[0]?.[0]).toBe("Open from terminal")
        expect(String(feedbackMock.showActionError.mock.calls[0]?.[1])).toContain("missing")
    })

    it("reveals the validated line and does not invent a pendingReveal column field", async () => {
        await openTerminalTarget(
            { kind: "file", path: "/ws/app.ts", line: 12, column: 4 },
            { triggerGroupIndex: 1 }
        )
        expect(useWorkspaceStore.getState().groups[1]?.activePath).toBe("/ws/app.ts")
        expect(useWorkspaceStore.getState().pendingReveal).toEqual({
            path: "/ws/app.ts",
            line: 12
        })
    })

    it("navigates http/https through built-in Preview and honors the singleton group", async () => {
        useWorkspaceStore.getState().openPreviewTab(0)
        await openTerminalTarget(
            { kind: "url", url: "https://example.com/docs" },
            { triggerGroupIndex: 1 }
        )
        expect(usePreviewStore.getState().navForWorkspace("/ws").url).toBe("https://example.com/docs")
        expect(useWorkspaceStore.getState().activeGroupIndex).toBe(0)
        expect(useWorkspaceStore.getState().groups[0]?.activePath).toBe(PREVIEW_TAB_PATH)
        expect(useWorkspaceStore.getState().groups[1]?.tabs.some((tab) => tab.path === PREVIEW_TAB_PATH)).toBe(false)
        expect(feedbackMock.showActionError).not.toHaveBeenCalled()
    })

    it("requires the current workspace key before Preview navigation", async () => {
        useWorkspaceStore.setState({ workspacePath: null })
        await openTerminalTarget({ kind: "url", url: "https://example.com" })
        expect(usePreviewStore.getState().nav["/ws"]).toBeUndefined()
        expect(feedbackMock.showActionError).toHaveBeenCalledTimes(1)
    })
})

describe("resolveHerdrTerminalBaseCwd", () => {
    const snapshot = {
        herdrSessionId: "work",
        protocol: 19,
        version: "0.8.0",
        spaces: [
            { id: "space-a", label: "A", order: 0, focused: true, path: "/spaces/a" },
            { id: "space-b", label: "B", order: 1, focused: false, path: "/spaces/b" }
        ],
        agents: [],
        tabs: [],
        terminals: [
            { terminalId: "term-1", paneId: "pane-1", workspaceId: "space-a", cwd: "/pane/exact" },
            { terminalId: "term-2", paneId: "pane-2", workspaceId: "space-b" }
        ],
        raw: {}
    } satisfies HerdrSnapshot

    it("prefers the exact named-session pane cwd and does not use another session snapshot", () => {
        expect(resolveHerdrTerminalBaseCwd({
            snapshot,
            terminalId: "term-1",
            paneId: "pane-1"
        })).toBe("/pane/exact")
    })

    it("rejects a mismatched pane identity and uses the exact terminal cwd", () => {
        expect(resolveHerdrTerminalBaseCwd({
            snapshot,
            terminalId: "term-2",
            paneId: "pane-1",
            workspaceId: "space-a"
        })).toBe("/spaces/b")
    })

    it("falls back to the owning Space path only", () => {
        expect(resolveHerdrTerminalBaseCwd({
            snapshot,
            terminalId: "term-2",
            paneId: "pane-2",
            workspaceId: "space-b"
        })).toBe("/spaces/b")
    })

    it("does not leak a selected-session snapshot or invent a relative cwd", () => {
        const selectedLeak = {
            ...snapshot,
            herdrSessionId: "other",
            terminals: [
                { terminalId: "term-1", paneId: "pane-1", cwd: "/selected/wrong" }
            ],
            spaces: [{ id: "space-a", label: "A", order: 0, focused: true, path: "/selected/space" }]
        } satisfies HerdrSnapshot
        expect(resolveHerdrTerminalBaseCwd({
            snapshot: null,
            terminalId: "term-1",
            paneId: "pane-1"
        })).toBeNull()
        expect(resolveHerdrTerminalBaseCwd({
            snapshot: selectedLeak,
            terminalId: "missing",
            paneId: "missing"
        })).toBeNull()
        expect(findTerminalTargetsInText("src/app.ts", null)).toEqual([])
    })
})

describe("createTerminalTargetContextGate", () => {
    function event(partial: Partial<MouseEvent> = {}) {
        return {
            button: 2,
            metaKey: true,
            ctrlKey: false,
            altKey: false,
            preventDefault: vi.fn(),
            stopPropagation: vi.fn(),
            ...partial
        }
    }

    it("keeps the pane menu after a left-click open", () => {
        const gate = createTerminalTargetContextGate()
        const fallback = vi.fn()
        const target = { kind: "file", path: "/tmp/a.ts" } as const
        gate.setHovered(target)
        gate.markConsumed(target)
        const next = event()
        expect(gate.handleContextMenu(next, fallback, { mac: true })).toBe(false)
        expect(next.preventDefault).not.toHaveBeenCalled()
        expect(fallback).not.toHaveBeenCalled()
    })

    it("keeps the ordinary menu for non-target modifier and ordinary right click", () => {
        const gate = createTerminalTargetContextGate()
        const activate = vi.fn()
        const ordinary = event({ metaKey: false })
        expect(gate.handleContextMenu(ordinary, activate, { mac: true })).toBe(false)
        expect(ordinary.preventDefault).not.toHaveBeenCalled()

        const modifierPlain = event()
        expect(gate.handleContextMenu(modifierPlain, activate, { mac: true })).toBe(false)
        expect(activate).not.toHaveBeenCalled()
    })

    it("does not open from a right-click context menu", () => {
        const gate = createTerminalTargetContextGate()
        const target: TerminalTarget = { kind: "url", url: "https://example.com/" }
        const activate = vi.fn()
        gate.setHovered(target)

        expect(gate.handleContextMenu(event(), activate, { mac: true })).toBe(false)
        expect(activate).not.toHaveBeenCalled()
    })

    it("disposes hovered and consume state with the owning xterm", () => {
        const gate = createTerminalTargetContextGate()
        const activate = vi.fn()
        const target = { kind: "file", path: "/tmp/a.ts" } as const
        gate.setHovered(target)
        gate.markConsumed(target)
        gate.dispose()
        const next = event()
        expect(gate.handleContextMenu(next, activate, { mac: true })).toBe(false)
        expect(activate).not.toHaveBeenCalled()
    })
})
