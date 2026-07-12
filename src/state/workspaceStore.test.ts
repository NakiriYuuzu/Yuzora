import { beforeEach, describe, expect, it } from "vitest"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "./workspaceStore"

const previewTabCount = () =>
    useWorkspaceStore
        .getState()
        .groups.reduce(
            (n, g) => n + g.tabs.filter((t) => t.path === PREVIEW_TAB_PATH).length,
            0
        )

const initialState = useWorkspaceStore.getState()

describe("workspaceStore", () => {
    beforeEach(() => {
        useWorkspaceStore.setState(initialState, true)
    })

    describe("setActiveGroup", () => {
        it("splitRight 後 setActiveGroup(1) 讓 openTab 開進 groups[1]", () => {
            useWorkspaceStore.getState().splitRight()
            useWorkspaceStore.getState().setActiveGroup(1)
            useWorkspaceStore.getState().openTab("/w/a.ts")

            const state = useWorkspaceStore.getState()
            expect(state.activeGroupIndex).toBe(1)
            expect(state.groups[1].tabs.map((t) => t.path)).toEqual(["/w/a.ts"])
            expect(state.groups[1].activePath).toBe("/w/a.ts")
            expect(state.groups[0].tabs).toEqual([])
        })

        it("越界 index 不變更 activeGroupIndex", () => {
            useWorkspaceStore.getState().setActiveGroup(99)

            expect(useWorkspaceStore.getState().activeGroupIndex).toBe(0)
        })
    })

    describe("right split atomic operations", () => {
        it("openInRightSplit uses the request snapshot, creates destination, and focuses it", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.setState({ activeGroupIndex: 99 })

            useWorkspaceStore.getState().openInRightSplit("/w/a.ts", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/a.ts"])
            expect(state.groups[1].activePath).toBe("/w/a.ts")
            expect(state.activeGroupIndex).toBe(1)
        })

        it("openInRightSplit reuses the right group and de-duplicates conservatively", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/a.ts",
                        tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }]
                    },
                    {
                        activePath: "/w/b.ts",
                        tabs: [
                            { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false },
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: true }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().openInRightSplit("/w/a.ts", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/b.ts", "/w/a.ts"])
            expect(state.groups[1].tabs[1]).toMatchObject({ dirty: true, externallyModified: true })
            expect(state.groups[1].activePath).toBe("/w/a.ts")
        })

        it("splitAndMoveRight moves the clicked tab object and preserves its state", () => {
            const clicked = {
                path: "/w/a.ts",
                name: "a.ts",
                dirty: true,
                externallyModified: true,
                kind: "file" as const
            }
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [{ activePath: clicked.path, tabs: [clicked] }]
            })

            useWorkspaceStore.getState().splitAndMoveRight(0, clicked.path)

            const state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[1].tabs[0]).toBe(clicked)
            expect(state.groups[1].activePath).toBe(clicked.path)
            expect(state.activeGroupIndex).toBe(1)
        })

        it("splitAndMoveRight de-duplicates an existing destination tab conservatively", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/a.ts",
                        tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }]
                    },
                    {
                        activePath: "/w/b.ts",
                        tabs: [
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: true },
                            { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().splitAndMoveRight(0, "/w/a.ts")

            const state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/a.ts", "/w/b.ts"])
            expect(state.groups[1].tabs[0]).toMatchObject({ dirty: true, externallyModified: true })
            expect(state.groups[1].activePath).toBe("/w/a.ts")
        })

        it("does not split the Preview sentinel or create a third/right-of-right group", () => {
            const preview = {
                path: PREVIEW_TAB_PATH,
                name: "Preview",
                dirty: false,
                externallyModified: false,
                kind: "preview" as const
            }
            useWorkspaceStore.setState({
                activeGroupIndex: 1,
                groups: [
                    { activePath: null, tabs: [] },
                    { activePath: preview.path, tabs: [preview] }
                ]
            })

            useWorkspaceStore.getState().splitAndMoveRight(1, preview.path)
            useWorkspaceStore.getState().openInRightSplit("/w/a.ts", 1)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[1].tabs).toEqual([preview])
        })

        it("openTab focuses an existing path in another group instead of creating a second view", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    { activePath: null, tabs: [] },
                    {
                        activePath: "/w/a.ts",
                        tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false }]
                    }
                ]
            })

            useWorkspaceStore.getState().openTab("/w/a.ts", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[1].tabs).toHaveLength(1)
            expect(state.activeGroupIndex).toBe(1)
        })
    })

    describe("preview tab (singleton)", () => {
        it("openPreviewTab opens the preview tab in the active group and focuses it", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().openPreviewTab()

            const s = useWorkspaceStore.getState()
            expect(previewTabCount()).toBe(1)
            expect(s.groups[0].activePath).toBe(PREVIEW_TAB_PATH)
            const preview = s.groups[0].tabs.find((t) => t.path === PREVIEW_TAB_PATH)
            expect(preview?.kind).toBe("preview")
            expect(preview?.dirty).toBe(false)
        })

        it("openPreviewTab is a singleton — a second call focuses the existing tab, no duplicate", () => {
            useWorkspaceStore.getState().splitRight()
            // preview lives in group 0
            useWorkspaceStore.getState().setActiveGroup(0)
            useWorkspaceStore.getState().openPreviewTab()
            // focus group 1, then ask for preview again
            useWorkspaceStore.getState().setActiveGroup(1)
            useWorkspaceStore.getState().openPreviewTab()

            const s = useWorkspaceStore.getState()
            expect(previewTabCount()).toBe(1)
            // Focus returns to the group that already holds the preview tab.
            expect(s.activeGroupIndex).toBe(0)
            expect(s.groups[0].activePath).toBe(PREVIEW_TAB_PATH)
        })

        it("closePreviewTab removes it and restores the previous tab as active", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().openPreviewTab()
            useWorkspaceStore.getState().closePreviewTab()

            const s = useWorkspaceStore.getState()
            expect(previewTabCount()).toBe(0)
            expect(s.groups[0].activePath).toBe("/w/a.ts")
        })

        it("togglePreviewTab closes when the preview tab is focused, opens/focuses otherwise", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            // not focused → opens
            useWorkspaceStore.getState().togglePreviewTab()
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(PREVIEW_TAB_PATH)
            // focused → closes
            useWorkspaceStore.getState().togglePreviewTab()
            expect(previewTabCount()).toBe(0)
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
        })

        it("togglePreviewTab focuses (not closes) an existing preview tab that isn't the active tab", () => {
            useWorkspaceStore.getState().openPreviewTab()
            // switch focus away to a file tab in the same group
            useWorkspaceStore.getState().openTab("/w/a.ts")
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
            // preview exists but isn't active → toggle focuses it, keeps the singleton
            useWorkspaceStore.getState().togglePreviewTab()
            expect(previewTabCount()).toBe(1)
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(PREVIEW_TAB_PATH)
        })
    })

    describe("closeTabsByPath", () => {
        it("closes matching tabs across all groups and re-picks activePath from survivors", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/b.ts",
                        tabs: [
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                            { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                        ]
                    },
                    {
                        activePath: "/w/a.ts",
                        tabs: [
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                            { path: "/w/c.ts", name: "c.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })
            useWorkspaceStore.getState().closeTabsByPath(["/w/b.ts", "/w/a.ts"])
            const s = useWorkspaceStore.getState()
            // group 0: both a and b closed → no survivors, activePath resets to null.
            expect(s.groups[0].tabs.map((t) => t.path)).toEqual([])
            expect(s.groups[0].activePath).toBeNull()
            // group 1: a (active) closed → activePath falls back to survivor c.
            expect(s.groups[1].tabs.map((t) => t.path)).toEqual(["/w/c.ts"])
            expect(s.groups[1].activePath).toBe("/w/c.ts")
        })

        it("leaves activePath untouched when the closed tabs weren't active", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/a.ts",
                        tabs: [
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                            { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })
            useWorkspaceStore.getState().closeTabsByPath(["/w/b.ts"])
            const s = useWorkspaceStore.getState()
            expect(s.groups[0].tabs.map((t) => t.path)).toEqual(["/w/a.ts"])
            expect(s.groups[0].activePath).toBe("/w/a.ts")
        })
    })

    describe("updateTabPath", () => {
        it("re-points a single renamed file's tab (path + name + activePath), preserving dirty", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/old.ts",
                        tabs: [
                            { path: "/w/old.ts", name: "old.ts", dirty: true, externallyModified: false },
                            { path: "/w/keep.ts", name: "keep.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })
            useWorkspaceStore.getState().updateTabPath("/w/old.ts", "/w/new.ts")
            const g = useWorkspaceStore.getState().groups[0]
            expect(g.tabs.map((t) => t.path)).toEqual(["/w/new.ts", "/w/keep.ts"])
            expect(g.tabs[0].name).toBe("new.ts")
            expect(g.tabs[0].dirty).toBe(true)
            expect(g.activePath).toBe("/w/new.ts")
        })

        it("remaps every tab under a renamed folder across groups (prefix rewrite)", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/dir/a.ts",
                        tabs: [
                            { path: "/w/dir/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                            { path: "/w/dir/sub/b.ts", name: "b.ts", dirty: false, externallyModified: false },
                            { path: "/w/other.ts", name: "other.ts", dirty: false, externallyModified: false }
                        ]
                    },
                    {
                        activePath: "/w/dir/sub/b.ts",
                        tabs: [
                            { path: "/w/dir/sub/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })
            useWorkspaceStore.getState().updateTabPath("/w/dir", "/w/renamed")
            const s = useWorkspaceStore.getState()
            expect(s.groups[0].tabs.map((t) => t.path)).toEqual([
                "/w/renamed/a.ts",
                "/w/renamed/sub/b.ts",
                "/w/other.ts"
            ])
            expect(s.groups[0].activePath).toBe("/w/renamed/a.ts")
            expect(s.groups[1].tabs.map((t) => t.path)).toEqual(["/w/renamed/sub/b.ts"])
            expect(s.groups[1].activePath).toBe("/w/renamed/sub/b.ts")
        })

        it("does not touch a sibling whose name merely shares the renamed prefix", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/foo.ts",
                        tabs: [
                            { path: "/w/foo.ts", name: "foo.ts", dirty: false, externallyModified: false },
                            { path: "/w/foobar.ts", name: "foobar.ts", dirty: false, externallyModified: false }
                        ]
                    }
                ]
            })
            useWorkspaceStore.getState().updateTabPath("/w/foo", "/w/baz")
            // Neither matches: /w/foo.ts !== /w/foo and doesn't start with "/w/foo/".
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
                "/w/foo.ts",
                "/w/foobar.ts"
            ])
        })
    })

    describe("requestReveal", () => {
        it("requestReveal opens tab and stores pending line", () => {
            useWorkspaceStore.getState().requestReveal("/w/a.ts", 42)
            const s = useWorkspaceStore.getState()
            expect(s.groups[s.activeGroupIndex].activePath).toBe("/w/a.ts")
            expect(s.pendingReveal).toEqual({ path: "/w/a.ts", line: 42 })
            useWorkspaceStore.getState().consumeReveal()
            expect(useWorkspaceStore.getState().pendingReveal).toBe(null)
        })
    })
})
