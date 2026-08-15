import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { markdownPreviewPath } from "@/lib/markdownPreviewTab"
import { useHerdrStore } from "@/state/herdrStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

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
    }) => (
        <div
            data-testid={`mock-herdr-${props.terminalId}`}
            data-active={String(props.active)}
            data-visible={String(props.visible)}
        />
    )
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
        expect(firstLayer.className).toContain("opacity-100")
        expect(secondLayer.className).toContain("opacity-0")
        expect(secondLayer.className).not.toContain("invisible")

        act(() => {
            useWorkspaceStore.getState().setActiveTab(0, secondPath)
        })

        expect(screen.getByTestId("mock-herdr-term-1")).toBe(first)
        expect(screen.getByTestId("mock-herdr-term-2")).toBe(second)
        expect(first).toHaveAttribute("data-active", "false")
        expect(first).toHaveAttribute("data-visible", "false")
        expect(second).toHaveAttribute("data-active", "true")
        expect(second).toHaveAttribute("data-visible", "true")
        expect(firstLayer.className).toContain("opacity-0")
        expect(secondLayer.className).toContain("opacity-100")
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
