import { useUiStore } from "@/state/uiStore"
import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { markdownPreviewPath } from "@/lib/markdownPreviewTab"
import { useHerdrStore } from "@/state/herdrStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

const herdrRender = vi.hoisted(() => vi.fn())

vi.mock("./TabBar", () => ({
    TabBar: ({ groupIndex }: { groupIndex: number }) => {
        const group = useWorkspaceStore((state) => state.groups[groupIndex])
        const selectedSpaceId = useHerdrStore((state) => state.selectedSpaceId)
        const selectedSessionName = useHerdrStore((state) => state.selectedSessionName)
        return (
            <div data-testid="tab-bar">
                {group?.tabs
                    .filter((tab) =>
                        tab.kind !== "herdr-terminal" ||
                        (tab.herdrSessionId === selectedSessionName &&
                            tab.herdrWorkspaceId === selectedSpaceId)
                    )
                    .map((tab) => <span key={tab.path}>{tab.name}</span>)}
            </div>
        )
    }
}))

vi.mock("@/app/panels/HerdrTerminalPage", () => ({
    HerdrTerminalPage: (props: {
        terminalId: string
        active: boolean
        visible?: boolean
    }) => {
        herdrRender(props.terminalId)
        return (
        <div
            data-testid={`mock-herdr-${props.terminalId}`}
            data-active={String(props.active)}
            data-visible={String(props.visible)}
        />
        )
    }
}))

vi.mock("@/app/panels/PreviewPanel", () => ({
    PreviewPanel: () => <div data-testid="preview-panel" />
}))

vi.mock("../editor/EditorPane", () => ({
    EditorPane: () => <div data-testid="editor-pane" />
}))

vi.mock("./MarkdownPreview", () => ({
    isMarkdownPath: (name: string) => name.endsWith(".md") || name.endsWith(".markdown"),
    MarkdownPreview: ({ sourcePath }: { sourcePath: string }) => (
        <div data-testid="markdown-preview" data-source={sourcePath} />
    )
}))

vi.mock("./ImageView", () => ({
    isImagePath: () => false,
    ImageView: () => <div data-testid="image-view" />
}))

vi.mock("./SvgSplitView", () => ({
    isSvgPath: () => false,
    SvgSplitView: () => <div data-testid="svg-split" />
}))

import { EditorArea } from "./EditorArea"

const initialWorkspaceState = useWorkspaceStore.getState()
const initialHerdrState = useHerdrStore.getState()

function herdrTab(path: string, terminalId: string, tabId: string) {
    return {
        path,
        name: tabId,
        dirty: false,
        externallyModified: false,
        kind: "herdr-terminal" as const,
        herdrSessionId: "default",
        terminalId,
        herdrTabId: tabId,
        paneId: `${terminalId}-pane`
    }
}

afterEach(() => {
    cleanup()
    useWorkspaceStore.setState(initialWorkspaceState, true)
    useHerdrStore.setState(initialHerdrState, true)
})

describe("EditorArea persistent Herdr pages", () => {
    it("updates only the departing and arriving terminal when switching among many open pages", () => {
        const tabs = Array.from({ length: 30 }, (_, i) => herdrTab(`yuzora://herdr/default/term-${i}`, `term-${i}`, `tab-${i}`))
        useUiStore.setState({ mode: "ade" })
        useWorkspaceStore.setState({ groups: [{ activePath: tabs[0].path, tabs }], activeGroupIndex: 0 })
        render(<EditorArea />)
        const hidden = screen.getByTestId("mock-herdr-term-29")
        herdrRender.mockClear()
        act(() => useWorkspaceStore.getState().setActiveTab(0, tabs[1].path))
        expect(herdrRender.mock.calls.map(([id]) => id)).toEqual(["term-0", "term-1"])
        expect(screen.getByTestId("mock-herdr-term-29")).toBe(hidden)
        expect(screen.getByTestId("mock-herdr-term-1")).toHaveAttribute("data-visible", "true")
    })

    it("switches the visible tab strip by Space without unmounting cached Herdr pages", () => {
        const firstPath = "yuzora://herdr/default/term-1"
        const secondPath = "yuzora://herdr/default/term-2"
        useWorkspaceStore.setState({
            groups: [{
                activePath: firstPath,
                tabs: [
                    { ...herdrTab(firstPath, "term-1", "tab-1"), name: "Space One", herdrWorkspaceId: "ws-1" },
                    { ...herdrTab(secondPath, "term-2", "tab-2"), name: "Space Two", herdrWorkspaceId: "ws-2" }
                ]
            }],
            activeGroupIndex: 0
        })
        useHerdrStore.setState({
            selectedSessionName: "default",
            selectedSpaceId: "ws-1"
        })

        render(<EditorArea />)
        const first = screen.getByTestId("mock-herdr-term-1")
        const second = screen.getByTestId("mock-herdr-term-2")
        expect(screen.getByText("Space One")).toBeInTheDocument()
        expect(screen.queryByText("Space Two")).not.toBeInTheDocument()

        act(() => {
            useHerdrStore.setState({ selectedSpaceId: "ws-2" })
        })

        expect(screen.queryByText("Space One")).not.toBeInTheDocument()
        expect(screen.getByText("Space Two")).toBeInTheDocument()
        expect(screen.getByTestId("mock-herdr-term-1")).toBe(first)
        expect(screen.getByTestId("mock-herdr-term-2")).toBe(second)
    })

    it("keeps inactive Herdr tabs mounted and only toggles visibility", () => {
        const firstPath = "yuzora://herdr/default/term-1"
        const secondPath = "yuzora://herdr/default/term-2"
        useWorkspaceStore.setState({
            groups: [
                {
                    activePath: firstPath,
                    tabs: [
                        herdrTab(firstPath, "term-1", "tab-1"),
                        herdrTab(secondPath, "term-2", "tab-2")
                    ]
                }
            ],
            activeGroupIndex: 0
        })

        render(<EditorArea />)

        const first = screen.getByTestId("mock-herdr-term-1")
        const second = screen.getByTestId("mock-herdr-term-2")
        const firstLayer = screen.getByTestId(`herdr-page-layer-${firstPath}`)
        const secondLayer = screen.getByTestId(`herdr-page-layer-${secondPath}`)
        expect(first).toHaveAttribute("data-active", "true")
        expect(first).toHaveAttribute("data-visible", "true")
        expect(second).toHaveAttribute("data-active", "false")
        expect(second).toHaveAttribute("data-visible", "false")
        expect(firstLayer.className).toContain("visible")
        expect(secondLayer.className).toContain("invisible")
        expect(secondLayer.className).not.toContain("transition-opacity")

        act(() => {
            useWorkspaceStore.getState().setActiveTab(0, secondPath)
        })

        expect(screen.getByTestId("mock-herdr-term-1")).toBe(first)
        expect(screen.getByTestId("mock-herdr-term-2")).toBe(second)
        expect(first).toHaveAttribute("data-active", "false")
        expect(first).toHaveAttribute("data-visible", "false")
        expect(second).toHaveAttribute("data-active", "true")
        expect(second).toHaveAttribute("data-visible", "true")
        expect(firstLayer.className).toContain("invisible")
        expect(secondLayer.className).toContain("visible")
    })
})

describe("EditorArea markdown preview tabs", () => {
    it("renders EditorPane for a markdown source and MarkdownPreview for the adjacent tab", () => {
        const previewPath = markdownPreviewPath("/w/readme.md")
        useWorkspaceStore.setState({
            groups: [
                {
                    activePath: "/w/readme.md",
                    tabs: [{ path: "/w/readme.md", name: "readme.md", dirty: false, externallyModified: false }]
                },
                {
                    activePath: previewPath,
                    tabs: [{
                        path: previewPath,
                        name: "Preview",
                        dirty: false,
                        externallyModified: false,
                        kind: "markdown-preview",
                        sourcePath: "/w/readme.md"
                    }]
                }
            ],
            activeGroupIndex: 1
        })
        render(<EditorArea />)
        expect(screen.getByTestId("editor-pane")).toBeInTheDocument()
        expect(screen.getByTestId("markdown-preview")).toHaveAttribute("data-source", "/w/readme.md")
        expect(screen.queryByTestId("markdown-split")).toBeNull()
        expect(screen.queryByTestId("preview-panel")).toBeNull()
    })

    it("still renders the browser PreviewPanel for the generic preview tab", () => {
        useWorkspaceStore.setState({
            groups: [{
                activePath: PREVIEW_TAB_PATH,
                tabs: [{
                    path: PREVIEW_TAB_PATH,
                    name: "Preview",
                    dirty: false,
                    externallyModified: false,
                    kind: "preview"
                }]
            }],
            activeGroupIndex: 0
        })
        render(<EditorArea />)
        expect(screen.getByTestId("preview-panel")).toBeInTheDocument()
        expect(screen.queryByTestId("markdown-preview")).toBeNull()
    })

    it("keeps the remaining Herdr page mounted when a preview-only group 0 is removed", () => {
        const previewPath = markdownPreviewPath("/w/readme.md")
        const herdrPath = "yuzora://herdr/default/term-keep"
        useWorkspaceStore.setState({
            groups: [
                {
                    id: "preview-only",
                    activePath: previewPath,
                    tabs: [{
                        path: previewPath,
                        name: "Preview",
                        dirty: false,
                        externallyModified: false,
                        kind: "markdown-preview",
                        sourcePath: "/w/readme.md"
                    }]
                },
                {
                    id: "herdr-keep",
                    activePath: herdrPath,
                    tabs: [{ ...herdrTab(herdrPath, "term-keep", "tab-keep"), herdrWorkspaceId: "ws-1" }]
                }
            ],
            activeGroupIndex: 0
        })
        useHerdrStore.setState({
            selectedSessionName: "default",
            selectedSpaceId: "ws-1"
        })

        render(<EditorArea />)
        const herdr = screen.getByTestId("mock-herdr-term-keep")
        act(() => {
            useWorkspaceStore.getState().closeMarkdownPreviewTab(0, previewPath)
        })
        expect(useWorkspaceStore.getState().groups).toHaveLength(1)
        expect(screen.getByTestId("mock-herdr-term-keep")).toBe(herdr)
    })

    it("keeps Herdr mounted when the browser preview tab in the same group is closed", () => {
        const herdrPath = "yuzora://herdr/default/term-keep"
        useWorkspaceStore.setState({
            groups: [{
                id: "mixed-preview-herdr",
                activePath: PREVIEW_TAB_PATH,
                tabs: [
                    {
                        path: PREVIEW_TAB_PATH,
                        name: "Preview",
                        dirty: false,
                        externallyModified: false,
                        kind: "preview"
                    },
                    { ...herdrTab(herdrPath, "term-keep", "tab-keep"), herdrWorkspaceId: "ws-1" }
                ]
            }],
            activeGroupIndex: 0
        })
        useHerdrStore.setState({
            selectedSessionName: "default",
            selectedSpaceId: "ws-1"
        })

        render(<EditorArea />)
        const herdr = screen.getByTestId("mock-herdr-term-keep")
        expect(screen.getByTestId("preview-panel")).toBeInTheDocument()
        act(() => {
            useWorkspaceStore.getState().closePreviewTab()
        })
        expect(useWorkspaceStore.getState().groups[0].id).toBe("mixed-preview-herdr")
        expect(screen.getByTestId("mock-herdr-term-keep")).toBe(herdr)
        expect(screen.queryByTestId("preview-panel")).toBeNull()
    })
})

 it("keeps Herdr mounted but invisible and inert across Git and Database modes", () => {
    const path = "yuzora://herdr/default/mode-terminal"
    useWorkspaceStore.setState({ groups: [{ activePath: path, tabs: [herdrTab(path, "mode-terminal", "mode-tab")] }], activeGroupIndex: 0 })
    useUiStore.getState().setMode("files")
    render(<EditorArea />)
    const terminal = screen.getByTestId("mock-herdr-mode-terminal")
    const layer = screen.getByTestId(`herdr-page-layer-${path}`)
    expect(terminal).toHaveAttribute("data-visible", "true")
    for (const mode of ["git", "database"] as const) {
        act(() => useUiStore.getState().setMode(mode))
        expect(screen.getByTestId("mock-herdr-mode-terminal")).toBe(terminal)
        expect(terminal).toHaveAttribute("data-visible", "false")
        expect(terminal).toHaveAttribute("data-active", "false")
        expect(layer).toHaveAttribute("inert")
    }
    act(() => useUiStore.getState().setMode("files"))
    expect(screen.getByTestId("mock-herdr-mode-terminal")).toBe(terminal)
    expect(terminal).toHaveAttribute("data-visible", "true")
    expect(terminal).toHaveAttribute("data-active", "true")
    expect(layer).not.toHaveAttribute("inert")
 })
