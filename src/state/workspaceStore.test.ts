import { beforeEach, describe, expect, it } from "vitest"
import { herdrPagePath } from "../lib/herdrPages"
import type { HerdrSnapshot, HerdrTabInfo } from "../lib/herdrTypes"
import { markdownPreviewPath } from "../lib/markdownPreviewTab"
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

    describe("path presentation", () => {
        it("keeps an extended Windows path as tab identity but stores only its basename", () => {
            const rawPath = String.raw`\\?\C:\Work\中文 workspace\a.ts`

            useWorkspaceStore.getState().openTab(rawPath)

            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                path: rawPath,
                name: "a.ts"
            })
        })

        it("keeps the renamed Windows tab basename sanitized", () => {
            const fromPath = String.raw`\\?\C:\Work\中文 workspace\a.ts`
            const toPath = String.raw`\\?\C:\Work\中文 workspace\b.ts`
            useWorkspaceStore.getState().openTab(fromPath)

            useWorkspaceStore.getState().updateTabPath(fromPath, toPath)

            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                path: toPath,
                name: "b.ts"
            })
        })
    })

    describe("line ending metadata", () => {
        it("hydrates detected metadata without dirtying the tab", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")

            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "crlf", 0)

            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: "crlf",
                dirty: false
            })
        })

        it("marks an explicit conversion dirty only when the value changes", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "lf", 0)

            useWorkspaceStore.getState().setLineEnding("/w/a.ts", "lf")
            expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(false)

            useWorkspaceStore.getState().setLineEnding("/w/a.ts", "crlf")
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: "crlf",
                dirty: true
            })
        })

        it("preserves metadata across split moves and rename", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "crlf", 0)

            useWorkspaceStore.getState().splitAndMoveRight(0, "/w/a.ts")
            useWorkspaceStore.getState().updateTabPath("/w/a.ts", "/w/b.ts")

            expect(useWorkspaceStore.getState().getLineEnding("/w/b.ts")).toBe("crlf")
            expect(useWorkspaceStore.getState().groups[1].tabs[0]).toMatchObject({
                path: "/w/b.ts",
                lineEnding: "crlf"
            })
        })

        it("preserves an explicit target on same-generation remount and replaces it after reload", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "mixed", 0)
            useWorkspaceStore.getState().setLineEnding("/w/a.ts", "crlf")

            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "mixed", 0)
            expect(useWorkspaceStore.getState().getLineEnding("/w/a.ts")).toBe("crlf")

            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "lf", 1)
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: "lf",
                lineEndingGeneration: 1
            })
        })

        it("clears editable metadata only for a newer non-editable generation", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "lf", 0)

            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", undefined, 1)
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: undefined,
                lineEndingGeneration: 1
            })

            useWorkspaceStore.getState().hydrateLineEnding("/w/a.ts", "lf", 1)
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: undefined,
                lineEndingGeneration: 1
            })
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

        it("closePreviewTab preserves the group's runtime-stable id", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            const groupId = useWorkspaceStore.getState().groups[0].id
            expect(groupId).toEqual(expect.any(String))
            useWorkspaceStore.getState().openPreviewTab()
            expect(useWorkspaceStore.getState().groups[0].id).toBe(groupId)
            useWorkspaceStore.getState().closePreviewTab()
            expect(useWorkspaceStore.getState().groups[0].id).toBe(groupId)
            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/a.ts"])
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

        it("remaps Windows drive folder descendants while preserving operational separators", () => {
            const fromDir = String.raw`C:\Work\Repo\src`
            const toDir = String.raw`C:\Work\Repo\lib`
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: String.raw`C:\Work\Repo\src\a.ts`,
                        tabs: [
                            {
                                path: String.raw`C:\Work\Repo\src\a.ts`,
                                name: "a.ts",
                                dirty: true,
                                externallyModified: false
                            },
                            {
                                path: String.raw`C:\Work\Repo\src\nested\b.ts`,
                                name: "b.ts",
                                dirty: false,
                                externallyModified: false
                            },
                            {
                                path: String.raw`C:\Work\Repo\outside.ts`,
                                name: "outside.ts",
                                dirty: false,
                                externallyModified: false
                            }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().updateTabPath(fromDir, toDir)
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
                String.raw`C:\Work\Repo\lib\a.ts`,
                String.raw`C:\Work\Repo\lib\nested\b.ts`,
                String.raw`C:\Work\Repo\outside.ts`
            ])
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
                String.raw`C:\Work\Repo\lib\a.ts`
            )
            expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
        })

        it("remaps Windows verbatim folder descendants without exposing slash-only matching", () => {
            const verbatimFrom = String.raw`\\?\C:\Work\Repo\src`
            const verbatimTo = String.raw`\\?\C:\Work\Repo\lib`
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: String.raw`\\?\C:\Work\Repo\src\nested\b.ts`,
                        tabs: [
                            {
                                path: String.raw`\\?\C:\Work\Repo\src\nested\b.ts`,
                                name: "b.ts",
                                dirty: false,
                                externallyModified: false
                            },
                            {
                                path: String.raw`C:\Work\Repo\src\a.ts`,
                                name: "a.ts",
                                dirty: true,
                                externallyModified: false
                            }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().updateTabPath(verbatimFrom, verbatimTo)
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
                String.raw`\\?\C:\Work\Repo\lib\nested\b.ts`,
                String.raw`\\?\C:\Work\Repo\lib\a.ts`
            ])
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
                String.raw`\\?\C:\Work\Repo\lib\nested\b.ts`
            )
        })

        it.each([
            [String.raw`C:\i̇`, String.raw`c:\İ\Camel.ts`],
            [String.raw`C:\İ`, String.raw`c:\i̇\Camel.ts`]
        ])("preserves the full filename when Windows case folding changes string length", (fromDir, tabPath) => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: tabPath,
                        tabs: [{
                            path: tabPath,
                            name: "Camel.ts",
                            dirty: true,
                            externallyModified: false
                        }]
                    }
                ]
            })

            useWorkspaceStore.getState().updateTabPath(fromDir, String.raw`C:\Renamed`)

            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                path: String.raw`C:\Renamed\Camel.ts`,
                name: "Camel.ts",
                dirty: true
            })
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
                String.raw`C:\Renamed\Camel.ts`
            )
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

    describe("herdr-terminal pages", () => {
        it("keeps open Herdr pages global without moving them between editor groups", () => {
            useWorkspaceStore.getState().setWorkspace("/workspace-a")
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-a",
                herdrTabId: "tab-a",
                title: "Agent A"
            })
            const firstPath = useWorkspaceStore.getState().groups[0].activePath!
            useWorkspaceStore.getState().splitAndMoveRight(0, firstPath)
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-b",
                herdrTabId: "tab-b",
                title: "Agent B",
                groupIndex: 0
            })
            const secondPath = useWorkspaceStore.getState().groups[0].activePath!
            useWorkspaceStore.getState().setActiveGroup(1)

            useWorkspaceStore.getState().setWorkspace("/workspace-b")

            const state = useWorkspaceStore.getState()
            expect(state.workspacePath).toBe("/workspace-b")
            expect(state.groups).toHaveLength(2)
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual([secondPath])
            expect(state.groups[0].activePath).toBe(secondPath)
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual([firstPath])
            expect(state.groups[1].activePath).toBe(firstPath)
            expect(state.activeGroupIndex).toBe(1)
        })

        it("still clears workspace-local file and preview pages on workspace change", () => {
            useWorkspaceStore.getState().setWorkspace("/workspace-a")
            useWorkspaceStore.getState().openTab("/workspace-a/a.ts")
            useWorkspaceStore.getState().openPreviewTab()

            useWorkspaceStore.getState().setWorkspace("/workspace-b")

            const groups = useWorkspaceStore.getState().groups
            expect(groups).toHaveLength(1)
            expect(groups[0]).toMatchObject({ tabs: [], activePath: null })
            expect(groups[0].id).toEqual(expect.any(String))
        })

        it("drops markdown preview tabs on workspace change", () => {
            useWorkspaceStore.getState().setWorkspace("/workspace-a")
            useWorkspaceStore.getState().openTab("/workspace-a/readme.md")
            useWorkspaceStore.getState().toggleMarkdownPreview("/workspace-a/readme.md", 0)
            expect(useWorkspaceStore.getState().hasMarkdownPreview("/workspace-a/readme.md")).toBe(true)

            useWorkspaceStore.getState().setWorkspace("/workspace-b")

            const groups = useWorkspaceStore.getState().groups
            expect(groups.length).toBeGreaterThanOrEqual(1)
            expect(groups.every((group) => group.tabs.length === 0)).toBe(true)
            expect(useWorkspaceStore.getState().hasMarkdownPreview("/workspace-a/readme.md")).toBe(false)
        })

        it("openHerdrTerminalPage creates a typed page keyed by (sessionId, terminalId)", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-1",
                title: "pi",
                paneId: "pane-1"
            })
            const tab = useWorkspaceStore.getState().groups[0].tabs[0]
            expect(tab.kind).toBe("herdr-terminal")
            expect(tab.herdrSessionId).toBe("default")
            expect(tab.terminalId).toBe("term-1")
            expect(tab.paneId).toBe("pane-1")
            expect(tab.path).toContain("default")
            expect(tab.path).toContain("term-1")
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(tab.path)
        })

        it("dedupes by (sessionId, terminalId) and focuses the existing page", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-1",
                title: "first"
            })
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-1",
                title: "second",
                paneId: "pane-moved"
            })
            const tabs = useWorkspaceStore.getState().groups[0].tabs.filter(
                (t) => t.kind === "herdr-terminal"
            )
            expect(tabs).toHaveLength(1)
            expect(tabs[0].name).toBe("second")
            expect(tabs[0].paneId).toBe("pane-moved")
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(tabs[0].path)
        })

        it("allows the same terminalId under different Herdr sessions", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "session-a",
                terminalId: "term-1"
            })
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "session-b",
                terminalId: "term-1"
            })
            const tabs = useWorkspaceStore.getState().groups[0].tabs.filter(
                (t) => t.kind === "herdr-terminal"
            )
            expect(tabs).toHaveLength(2)
            expect(tabs.map((t) => t.herdrSessionId).sort()).toEqual(["session-a", "session-b"])
        })


        it("dedupes by (sessionId, tabId) when herdrTabId is present", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-1",
                herdrTabId: "tab-a",
                title: "first"
            })
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-2",
                herdrTabId: "tab-a",
                title: "second pane"
            })
            const tabs = useWorkspaceStore.getState().groups[0].tabs.filter(
                (t) => t.kind === "herdr-terminal"
            )
            expect(tabs).toHaveLength(1)
            expect(tabs[0].herdrTabId).toBe("tab-a")
            expect(tabs[0].name).toBe("second pane")
            // path keeps the first terminal identity
            expect(tabs[0].terminalId).toBe("term-1")
        })

        it("falls back to terminalId dedupe for legacy pages without tabId", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-legacy",
                title: "legacy"
            })
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-legacy",
                herdrTabId: "tab-new",
                title: "upgraded"
            })
            const tabs = useWorkspaceStore.getState().groups[0].tabs.filter(
                (t) => t.kind === "herdr-terminal"
            )
            expect(tabs).toHaveLength(1)
            expect(tabs[0].herdrTabId).toBe("tab-new")
            expect(tabs[0].name).toBe("upgraded")
        })

        it("updateHerdrPagePaneId mutates pane metadata without changing identity", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-1",
                paneId: "pane-old"
            })
            const path = useWorkspaceStore.getState().groups[0].tabs[0].path
            useWorkspaceStore.getState().updateHerdrPagePaneId(path, "pane-new")
            const tab = useWorkspaceStore.getState().groups[0].tabs[0]
            expect(tab.path).toBe(path)
            expect(tab.paneId).toBe("pane-new")
            expect(tab.terminalId).toBe("term-1")
        })

        it("upgrades legacy tab identity and synchronizes an explicit Herdr tab rename", () => {
            useWorkspaceStore.getState().openHerdrTerminalPage({
                herdrSessionId: "default",
                terminalId: "term-legacy",
                title: "Old name"
            })
            const path = useWorkspaceStore.getState().groups[0].tabs[0].path
            useWorkspaceStore.getState().updateHerdrPageTabId(path, "tab-resolved")
            useWorkspaceStore.getState().updateHerdrPageTitle(path, "Renamed")
            const tab = useWorkspaceStore.getState().groups[0].tabs[0]
            expect(tab.path).toBe(path)
            expect(tab.herdrTabId).toBe("tab-resolved")
            expect(tab.name).toBe("Renamed")
        })
    })

    describe("reorderTab", () => {
        it("moves a tab by identity and keeps activePath plus metadata", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().openTab("/w/b.ts")
            useWorkspaceStore.getState().openTab("/w/c.ts")
            useWorkspaceStore.getState().markDirty("/w/a.ts", true)
            useWorkspaceStore.getState().setActiveTab(0, "/w/a.ts")

            useWorkspaceStore.getState().reorderTab(0, "/w/a.ts", 2)

            const group = useWorkspaceStore.getState().groups[0]
            expect(group.tabs.map((tab) => tab.path)).toEqual(["/w/b.ts", "/w/c.ts", "/w/a.ts"])
            expect(group.activePath).toBe("/w/a.ts")
            expect(group.tabs[2]).toMatchObject({ path: "/w/a.ts", dirty: true })
        })

        it("no-ops invalid path and same-index destination", () => {
            useWorkspaceStore.getState().openTab("/w/a.ts")
            useWorkspaceStore.getState().openTab("/w/b.ts")
            const before = useWorkspaceStore.getState().groups[0].tabs

            useWorkspaceStore.getState().reorderTab(0, "/w/missing.ts", 1)
            useWorkspaceStore.getState().reorderTab(0, "/w/a.ts", 0)

            expect(useWorkspaceStore.getState().groups[0].tabs).toBe(before)
        })

        it("projected reorder keeps hidden-Space pages in their slots", () => {
            const hidden = {
                path: "yuzora://herdr/default/hidden",
                name: "Hidden",
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal" as const,
                herdrSessionId: "default",
                herdrTabId: "tab-h",
                herdrWorkspaceId: "ws-2"
            }
            const a = { path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }
            const b = { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
            useWorkspaceStore.setState({
                groups: [{ activePath: "/w/a.ts", tabs: [a, hidden, b] }]
            })
            const projected = [a, b]

            useWorkspaceStore.getState().reorderProjectedTab(0, "/w/a.ts", 1, projected)

            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
                "/w/b.ts",
                "yuzora://herdr/default/hidden",
                "/w/a.ts"
            ])
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
            expect(useWorkspaceStore.getState().groups[0].tabs[2]).toMatchObject({
                path: "/w/a.ts",
                dirty: true
            })
        })
    })

    describe("reconcileHerdrPagesFromSnapshot", () => {
        it("reorders only same-Space Herdr pages and leaves files plus hidden Spaces", () => {
            useWorkspaceStore.setState({
                groups: [
                    {
                        activePath: "/w/a.ts",
                        tabs: [
                            { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                            {
                                path: "yuzora://herdr/default/t1",
                                name: "One",
                                dirty: false,
                                externallyModified: false,
                                kind: "herdr-terminal",
                                herdrSessionId: "default",
                                herdrTabId: "tab-1",
                                herdrWorkspaceId: "ws-1"
                            },
                            {
                                path: "yuzora://herdr/default/t2",
                                name: "Two",
                                dirty: false,
                                externallyModified: false,
                                kind: "herdr-terminal",
                                herdrSessionId: "default",
                                herdrTabId: "tab-2",
                                herdrWorkspaceId: "ws-1"
                            },
                            {
                                path: "yuzora://herdr/default/hidden",
                                name: "Hidden",
                                dirty: false,
                                externallyModified: false,
                                kind: "herdr-terminal",
                                herdrSessionId: "default",
                                herdrTabId: "tab-h",
                                herdrWorkspaceId: "ws-2"
                            }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot({
                herdrSessionId: "default",
                protocol: 19,
                version: "0.8.0",
                spaces: [],
                agents: [],
                tabs: [
                    {
                        id: "tab-2",
                        label: "Two",
                        order: 0,
                        workspaceId: "ws-1",
                        paneCount: 1,
                        status: "idle",
                        active: true,
                        focused: true
                    },
                    {
                        id: "tab-1",
                        label: "One",
                        order: 1,
                        workspaceId: "ws-1",
                        paneCount: 1,
                        status: "idle",
                        active: false,
                        focused: false
                    }
                ],
                terminals: [],
                raw: {}
            })

            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
                "/w/a.ts",
                "yuzora://herdr/default/t2",
                "yuzora://herdr/default/t1",
                "yuzora://herdr/default/hidden"
            ])
        })

        it("reconciles legacy page ownership from the same-session snapshot without moving hidden slots", () => {
            const legacyPage = (session: string, suffix: string, tabId: string) => ({
                path: `yuzora://herdr/${session}/${suffix}`,
                name: `${session}-${suffix}`,
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal" as const,
                herdrSessionId: session,
                herdrTabId: tabId
            })
            const hiddenPage = {
                ...legacyPage("live", "hidden", "tab-hidden"),
                herdrWorkspaceId: "ws-hidden"
            }
            useWorkspaceStore.setState({
                groups: [{
                    activePath: "/w/a.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        legacyPage("live", "live-1", "tab-1"),
                        hiddenPage,
                        legacyPage("live", "live-2", "tab-2"),
                        legacyPage("work", "work-1", "tab-1")
                    ]
                }]
            })

            useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot({
                herdrSessionId: "default",
                protocol: 19,
                version: "0.8.0",
                spaces: [],
                agents: [],
                tabs: [
                    {
                        id: "tab-2",
                        label: "Two",
                        order: 0,
                        workspaceId: "ws-1",
                        paneCount: 1,
                        status: "idle",
                        active: true,
                        focused: true
                    },
                    {
                        id: "tab-1",
                        label: "One",
                        order: 1,
                        workspaceId: "ws-1",
                        paneCount: 1,
                        status: "idle",
                        active: false,
                        focused: false
                    }
                ],
                terminals: [],
                raw: {}
            }, "default")

            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
                "/w/a.ts",
                "yuzora://herdr/live/live-2",
                "yuzora://herdr/live/hidden",
                "yuzora://herdr/live/live-1",
                "yuzora://herdr/work/work-1"
            ])
            expect(useWorkspaceStore.getState().groups[0].tabs[2]).toEqual({
                ...hiddenPage,
                herdrRuntimeTarget: { kind: "native" }
            })
        })

        it("keeps colliding Space and tab ids isolated between live/default and another named session", () => {
            const page = (session: string, suffix: string, tabId: string) => ({
                path: `yuzora://herdr/${session}/${suffix}`,
                name: `${session}-${suffix}`,
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal" as const,
                herdrSessionId: session,
                herdrTabId: tabId,
                herdrWorkspaceId: "ws-collision"
            })
            useWorkspaceStore.setState({
                groups: [{
                    activePath: null,
                    tabs: [
                        page("live", "live-1", "tab-1"),
                        page("work", "work-1", "tab-1"),
                        page("live", "live-2", "tab-2"),
                        page("work", "work-2", "tab-2")
                    ]
                }]
            })
            const snapshot = (sessionName: string) => ({
                herdrSessionId: sessionName,
                protocol: 19,
                version: "0.8.0",
                spaces: [],
                agents: [],
                tabs: [
                    {
                        id: "tab-2",
                        label: "Two",
                        order: 0,
                        workspaceId: "ws-collision",
                        paneCount: 1,
                        status: "idle" as const,
                        active: true,
                        focused: true
                    },
                    {
                        id: "tab-1",
                        label: "One",
                        order: 1,
                        workspaceId: "ws-collision",
                        paneCount: 1,
                        status: "idle" as const,
                        active: false,
                        focused: false
                    }
                ],
                terminals: [],
                raw: {}
            })

            useWorkspaceStore
                .getState()
                .reconcileHerdrPagesFromSnapshot(snapshot("work"), "default")
            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
                "yuzora://herdr/live/live-1",
                "yuzora://herdr/work/work-2",
                "yuzora://herdr/live/live-2",
                "yuzora://herdr/work/work-1"
            ])

            useWorkspaceStore
                .getState()
                .reconcileHerdrPagesFromSnapshot(snapshot("default"), "default")
            expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
                "yuzora://herdr/live/live-2",
                "yuzora://herdr/work/work-2",
                "yuzora://herdr/live/live-1",
                "yuzora://herdr/work/work-1"
            ])
        })
    })

    describe("hydrateHerdrPagesFromSnapshot", () => {
        const page = (
            session: string,
            terminalId: string,
            tabId?: string | null,
            workspaceId?: string | null
        ) => ({
            path: herdrPagePath(session, terminalId),
            name: `${session}-${terminalId}`,
            dirty: false,
            externallyModified: false,
            kind: "herdr-terminal" as const,
            herdrSessionId: session,
            terminalId,
            herdrTabId: tabId ?? null,
            herdrWorkspaceId: workspaceId ?? null
        })

        const snapshotTab = (
            id: string,
            workspaceId: string,
            terminalId: string | null,
            extras: Partial<HerdrTabInfo> = {}
        ): HerdrTabInfo => ({
            id,
            label: extras.label ?? id,
            order: extras.order ?? 0,
            workspaceId,
            paneCount: extras.paneCount ?? 1,
            status: extras.status ?? "idle",
            active: extras.active ?? id === "tab-2",
            focused: extras.focused ?? id === "tab-2",
            terminalId,
            paneId: extras.paneId ?? null,
            sessionName: extras.sessionName
        })

        const snapshot = (overrides: Partial<HerdrSnapshot> = {}): HerdrSnapshot => ({
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [],
            agents: [],
            terminals: [],
            raw: {},
            focusedWorkspaceId: "ws-1",
            focusedTabId: "tab-2",
            focusedPaneId: "pane-2",
            tabs: [
                snapshotTab("tab-1", "ws-1", "term-1", { label: "One", order: 0, paneId: "pane-1" }),
                snapshotTab("tab-2", "ws-1", "term-2", {
                    label: "Two",
                    order: 1,
                    focused: true,
                    paneId: "pane-2",
                    paneCount: 3
                }),
                snapshotTab("tab-3", "ws-1", "term-3", { label: "Three", order: 2, paneId: "pane-3" }),
                snapshotTab("tab-hidden", "ws-2", "term-hidden", { label: "Other space" }),
                snapshotTab("tab-empty", "ws-1", null, { label: "No terminal" })
            ],
            ...overrides
        })

        it("creates every usable focused-Space tab in snapshot order and keeps the focused tab active", () => {
            const hidden = page("default", "hidden", "tab-hidden", "ws-2")
            const otherSession = page("work", "term-1", "tab-1", "ws-1")
            useWorkspaceStore.setState({
                groups: [{
                    activePath: "/w/a.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        hidden,
                        otherSession
                    ]
                }]
            })

            useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(), "default")
            const group = useWorkspaceStore.getState().groups[0]
            expect(group.tabs.map((tab) => tab.path)).toEqual([
                "/w/a.ts",
                hidden.path,
                otherSession.path,
                herdrPagePath("default", "term-1"),
                herdrPagePath("default", "term-2"),
                herdrPagePath("default", "term-3")
            ])
            expect(group.activePath).toBe(herdrPagePath("default", "term-2"))
            expect(group.tabs.filter((tab) => tab.herdrWorkspaceId === "ws-1" && tab.herdrSessionId === "default")).toHaveLength(3)
            expect(group.tabs.some((tab) => tab.herdrTabId === "tab-hidden" && tab.path === hidden.path)).toBe(true)
            expect(group.tabs.filter((tab) => tab.path === otherSession.path)).toHaveLength(1)
            expect(group.tabs[1]).toBe(hidden)
            expect(group.tabs.filter((tab) => tab.herdrTabId === "tab-2")).toHaveLength(1)
        })

        it("is idempotent and does not duplicate or close existing pages", () => {
            useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(), "default")
            useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(), "default")
            const tabs = useWorkspaceStore.getState().groups[0].tabs
            expect(tabs.map((tab) => tab.herdrTabId)).toEqual(["tab-1", "tab-2", "tab-3"])
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe(
                herdrPagePath("default", "term-2")
            )
        })

        it("reuses live/default pages and leaves colliding named-session pages isolated", () => {
            const liveOne = page("live", "legacy-1", "tab-1")
            const liveTwo = page("live", "legacy-2", "tab-3")
            const workTwin = page("work", "work-1", "tab-1", "ws-1")
            const hidden = page("live", "hidden", "tab-hidden", "ws-2")
            useWorkspaceStore.setState({
                groups: [{
                    activePath: liveOne.path,
                    tabs: [liveOne, hidden, liveTwo, workTwin]
                }]
            })

            useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(), "default")
            const group = useWorkspaceStore.getState().groups[0]
            expect(group.tabs.map((tab) => tab.path)).toEqual([
                liveOne.path,
                hidden.path,
                herdrPagePath("default", "term-2"),
                workTwin.path,
                liveTwo.path
            ])
            expect(group.activePath).toBe(herdrPagePath("default", "term-2"))
            expect(group.tabs.filter((tab) => tab.herdrSessionId === "work")).toEqual([workTwin])
            expect(group.tabs.filter((tab) => tab.herdrTabId === "tab-1")).toHaveLength(2)
        })
    })

    describe("markdown preview editor-group tabs", () => {
        const fileTab = (path: string, dirty = false) => ({
            path,
            name: path.split("/").at(-1) ?? path,
            dirty,
            externallyModified: false
        })

        function seedSource(groupIndex = 0, extraGroups: Array<{ activePath: string | null; tabs: ReturnType<typeof fileTab>[] }> = []) {
            useWorkspaceStore.setState({
                activeGroupIndex: groupIndex,
                groups: [
                    { activePath: "/w/readme.md", tabs: [fileTab("/w/readme.md", true)] },
                    ...extraGroups
                ]
            })
        }

        it("opens from g0 into a new g1 without moving the source", () => {
            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/readme.md"])
            expect(state.groups[0].tabs[0].dirty).toBe(true)
            expect(state.groups[1].tabs).toHaveLength(1)
            expect(state.groups[1].tabs[0]).toMatchObject({
                kind: "markdown-preview",
                sourcePath: "/w/readme.md",
                name: "Preview",
                dirty: false
            })
            expect(state.groups[1].activePath).toBe(state.groups[1].tabs[0].path)
            expect(state.activeGroupIndex).toBe(1)
        })

        it("appends into an existing g1 and leaves source identity/position", () => {
            seedSource(0, [{
                activePath: "/w/notes.md",
                tabs: [fileTab("/w/notes.md")]
            }])
            const sourceBefore = useWorkspaceStore.getState().groups[0].tabs[0]
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[0].tabs[0]).toBe(sourceBefore)
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual([
                "/w/notes.md",
                state.groups[1].tabs[1].path
            ])
            expect(state.groups[1].tabs[1].sourcePath).toBe("/w/readme.md")
            expect(state.activeGroupIndex).toBe(1)
        })

        it("opens from g1 into g0 and never creates a third group", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 1,
                groups: [
                    { activePath: "/w/a.ts", tabs: [fileTab("/w/a.ts")] },
                    { activePath: "/w/readme.md", tabs: [fileTab("/w/readme.md")] }
                ]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 1)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/readme.md"])
            expect(state.groups[0].tabs[1]).toMatchObject({
                kind: "markdown-preview",
                sourcePath: "/w/readme.md"
            })
            expect(state.activeGroupIndex).toBe(0)
        })

        it("dedupes same-source and moves a misplaced preview to the adjacent group", () => {
            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            const previewPath = useWorkspaceStore.getState().groups[1].tabs[0].path
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            expect(useWorkspaceStore.getState().groups).toHaveLength(1)

            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/readme.md",
                        tabs: [
                            fileTab("/w/readme.md"),
                            {
                                path: previewPath,
                                name: "Preview",
                                dirty: false,
                                externallyModified: false,
                                kind: "markdown-preview",
                                sourcePath: "/w/readme.md"
                            }
                        ]
                    },
                    { activePath: "/w/a.ts", tabs: [fileTab("/w/a.ts")] }
                ]
            })
            useWorkspaceStore.getState().openMarkdownPreviewInAdjacentGroup("/w/readme.md", 0)

            const state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/readme.md"])
            expect(state.groups[1].tabs.filter((tab) => tab.kind === "markdown-preview")).toHaveLength(1)
            expect(state.groups[1].activePath).toBe(previewPath)
        })

        it("allows multiple previews for different sources in the same destination", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [{
                    activePath: "/w/readme.md",
                    tabs: [fileTab("/w/readme.md"), fileTab("/w/notes.md")]
                }]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/notes.md", 0)

            const dest = useWorkspaceStore.getState().groups[1]
            expect(dest.tabs.map((tab) => tab.sourcePath)).toEqual(["/w/readme.md", "/w/notes.md"])
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            expect(useWorkspaceStore.getState().groups[1].tabs.map((tab) => tab.sourcePath)).toEqual(["/w/notes.md"])
        })

        it("closes only the preview tab when the group still has other tabs", () => {
            seedSource(0, [{ activePath: "/w/a.ts", tabs: [fileTab("/w/a.ts")] }])
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            const previewPath = useWorkspaceStore.getState().groups[1].tabs[1].path
            useWorkspaceStore.getState().closeMarkdownPreviewTab(1, previewPath)

            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(2)
            expect(state.groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/a.ts"])
        })

        it("removes a preview-only group and never collapses to zero groups", () => {
            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            const previewPath = useWorkspaceStore.getState().groups[1].tabs[0].path
            useWorkspaceStore.getState().closeMarkdownPreviewTab(1, previewPath)
            expect(useWorkspaceStore.getState().groups).toHaveLength(1)
            expect(useWorkspaceStore.getState().activeGroupIndex).toBe(0)

            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [{
                    activePath: previewPath,
                    tabs: [{
                        path: previewPath,
                        name: "Preview",
                        dirty: false,
                        externallyModified: false,
                        kind: "markdown-preview",
                        sourcePath: "/w/readme.md"
                    }]
                }]
            })
            useWorkspaceStore.getState().closeMarkdownPreviewTab(0, previewPath)
            expect(useWorkspaceStore.getState().groups).toHaveLength(1)
            expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
        })

        it("source close/delete/rename keep preview tabs linked", () => {
            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            useWorkspaceStore.getState().closeTab(0, "/w/readme.md")
            expect(useWorkspaceStore.getState().groups).toHaveLength(1)
            expect(useWorkspaceStore.getState().groups[0].tabs.some((tab) => tab.kind === "markdown-preview")).toBe(false)

            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            useWorkspaceStore.getState().closeTabsByPath(["/w/readme.md"])
            expect(useWorkspaceStore.getState().groups).toHaveLength(1)

            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            useWorkspaceStore.getState().updateTabPath("/w/readme.md", "/w/renamed.md")
            const preview = useWorkspaceStore.getState().groups[1].tabs[0]
            expect(preview.sourcePath).toBe("/w/renamed.md")
            expect(preview.path).toContain(encodeURIComponent("/w/renamed.md"))
            expect(useWorkspaceStore.getState().groups[1].activePath).toBe(preview.path)

            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [{
                    activePath: "/w/dir/a.md",
                    tabs: [fileTab("/w/dir/a.md")]
                }]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/dir/a.md", 0)
            useWorkspaceStore.getState().updateTabPath("/w/dir", "/w/renamed-dir")
            expect(useWorkspaceStore.getState().groups[0].tabs[0].path).toBe("/w/renamed-dir/a.md")
            expect(useWorkspaceStore.getState().groups[1].tabs[0].sourcePath).toBe("/w/renamed-dir/a.md")
        })

        it("does not split markdown preview tabs", () => {
            seedSource()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            const previewPath = useWorkspaceStore.getState().groups[1].tabs[0].path
            useWorkspaceStore.getState().splitAndMoveRight(1, previewPath)
            useWorkspaceStore.getState().openInRightSplit(previewPath, 1)
            expect(useWorkspaceStore.getState().groups).toHaveLength(2)
            expect(useWorkspaceStore.getState().groups[1].tabs[0].path).toBe(previewPath)
        })

        it("rehomes the linked preview when the source moves between groups", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [{
                    activePath: "/w/readme.md",
                    tabs: [fileTab("/w/readme.md"), fileTab("/w/other.md")]
                }]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/other.md", 0)
            const otherPreview = useWorkspaceStore.getState().groups[1].tabs.find(
                (tab) => tab.sourcePath === "/w/other.md"
            )

            const readmePreviewPath = markdownPreviewPath("/w/readme.md")
            useWorkspaceStore.getState().splitAndMoveRight(0, "/w/readme.md")
            let state = useWorkspaceStore.getState()
            expect(state.groups[1].tabs.map((tab) => tab.path)).toContain("/w/readme.md")
            expect(state.groups[0].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(true)
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(false)
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/other.md")).toBe(true)
            expect(otherPreview && state.groups[1].tabs.some((tab) => tab.path === otherPreview.path)).toBe(true)
            expect(state.groups[0].tabs.map((tab) => tab.path)).toContain("/w/other.md")
            expect(state.groups[0].activePath).toBe(readmePreviewPath)

            useWorkspaceStore.getState().openTabInGroup("/w/readme.md", 0)
            state = useWorkspaceStore.getState()
            expect(state.groups[0].tabs.map((tab) => tab.path)).toContain("/w/readme.md")
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(true)
            expect(state.groups[0].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(false)
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/other.md")).toBe(true)
            expect(state.groups[1].activePath).toBe(readmePreviewPath)

            useWorkspaceStore.getState().openInRightSplit("/w/readme.md", 0)
            state = useWorkspaceStore.getState()
            expect(state.groups[1].tabs.map((tab) => tab.path)).toContain("/w/readme.md")
            expect(state.groups[0].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(true)
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(false)
            expect(state.groups[1].tabs.some((tab) => tab.sourcePath === "/w/other.md")).toBe(true)
            expect(state.groups[0].activePath).toBe(readmePreviewPath)
        })

        it("closeSplit removes orphan previews for discarded g1 sources", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 1,
                groups: [
                    { activePath: "/w/keep.ts", tabs: [fileTab("/w/keep.ts")] },
                    { activePath: "/w/readme.md", tabs: [fileTab("/w/readme.md")] }
                ]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 1)
            expect(useWorkspaceStore.getState().groups[0].tabs.some((tab) => tab.sourcePath === "/w/readme.md")).toBe(true)

            useWorkspaceStore.getState().closeSplit()
            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(1)
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/keep.ts"])
            expect(state.groups[0].tabs.some((tab) => tab.kind === "markdown-preview")).toBe(false)
        })

        it("toggle atomically closes every same-source duplicate", () => {
            const previewA = {
                path: "yuzora://markdown-preview/%2Fw%2Freadme.md",
                name: "Preview",
                dirty: false,
                externallyModified: false,
                kind: "markdown-preview" as const,
                sourcePath: "/w/readme.md"
            }
            const previewB = {
                ...previewA,
                path: "yuzora://markdown-preview/dup-readme"
            }
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
                groups: [
                    {
                        activePath: "/w/readme.md",
                        tabs: [fileTab("/w/readme.md"), previewA]
                    },
                    {
                        activePath: previewB.path,
                        tabs: [previewB, fileTab("/w/keep.ts")]
                    }
                ]
            })

            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            const state = useWorkspaceStore.getState()
            expect(state.groups.every((group) =>
                !group.tabs.some((tab) => tab.sourcePath === "/w/readme.md" && tab.kind === "markdown-preview")
            )).toBe(true)
            expect(state.groups.some((group) => group.tabs.some((tab) => tab.path === "/w/keep.ts"))).toBe(true)
        })

        it("closeSplit prunes a leftover preview-only g0 and never yields zero groups", () => {
            useWorkspaceStore.setState({
                activeGroupIndex: 1,
                groups: [
                    { activePath: null, tabs: [] },
                    { activePath: "/w/readme.md", tabs: [fileTab("/w/readme.md")] }
                ]
            })
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 1)
            expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
            expect(useWorkspaceStore.getState().groups[0].tabs[0].kind).toBe("markdown-preview")

            useWorkspaceStore.getState().closeSplit()
            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(1)
            expect(state.groups[0].tabs).toEqual([])
            expect(state.groups[0].id).toEqual(expect.any(String))
        })

        it("preserves the remaining group's runtime id after preview-only g0 removal", () => {
            const previewPath = markdownPreviewPath("/w/readme.md")
            const herdrPath = herdrPagePath("default", "term-keep")
            useWorkspaceStore.setState({
                activeGroupIndex: 0,
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
                        id: "source-keep",
                        activePath: "/w/readme.md",
                        tabs: [
                            fileTab("/w/readme.md"),
                            {
                                path: herdrPath,
                                name: "Keep",
                                dirty: false,
                                externallyModified: false,
                                kind: "herdr-terminal",
                                herdrSessionId: "default",
                                terminalId: "term-keep",
                                herdrTabId: "tab-keep"
                            }
                        ]
                    }
                ]
            })

            useWorkspaceStore.getState().closeMarkdownPreviewTab(0, previewPath)
            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(1)
            expect(state.groups[0].id).toBe("source-keep")
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/readme.md", herdrPath])
        })

        it("legacy groups without id still open and close preview without collapsing to zero groups", () => {
            seedSource()
            expect(useWorkspaceStore.getState().groups[0].id).toBeUndefined()
            useWorkspaceStore.getState().toggleMarkdownPreview("/w/readme.md", 0)
            expect(useWorkspaceStore.getState().groups[1].id).toEqual(expect.any(String))

            const previewPath = useWorkspaceStore.getState().groups[1].tabs[0].path
            useWorkspaceStore.getState().closeMarkdownPreviewTab(1, previewPath)
            const state = useWorkspaceStore.getState()
            expect(state.groups).toHaveLength(1)
            expect(state.groups[0].id).toBeUndefined()
            expect(state.groups[0].tabs.map((tab) => tab.path)).toEqual(["/w/readme.md"])
        })
    })
})
