import { describe, expect, it } from "vitest"

import type { TabInfo } from "@/state/workspaceStore"

import { normalizeHerdrSnapshot } from "./herdrNormalize"
import type { HerdrSnapshot, HerdrTabInfo } from "./herdrTypes"
import { herdrPagePath } from "./herdrPages"
import {
    herdrInsertIndexForProjectedDrop,
    herdrPageMatchesSnapshotSession,
    hydrateFocusedSpaceHerdrPages,
    moveItemToIndex,
    reorderProjectedSlots,
    resolveSpaceTabCount
} from "./workbenchTabReorder"

const file = (path: string): TabInfo => ({
    path,
    name: path,
    dirty: false,
    externallyModified: false
})

const herdr = (path: string, workspaceId: string): TabInfo => ({
    path,
    name: path,
    dirty: false,
    externallyModified: false,
    kind: "herdr-terminal",
    herdrTabId: path,
    herdrWorkspaceId: workspaceId
})

describe("workbenchTabReorder", () => {
    it("moves items with terminal-drawer splice semantics", () => {
        expect(moveItemToIndex(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"])
        expect(moveItemToIndex(["a", "b", "c"], 0, 0)).toBeNull()
        expect(moveItemToIndex(["a", "b", "c"], 3, 1)).toBeNull()
    })

    it("reorders only projected slots and preserves hidden-Space pages", () => {
        const group = [file("/a"), herdr("h-hidden", "ws-2"), file("/b")]
        const projected = [file("/a"), file("/b")]
        expect(reorderProjectedSlots(group, projected, "/a", 1)?.map((tab) => tab.path)).toEqual([
            "/b",
            "h-hidden",
            "/a"
        ])
    })

    it("matches live pages only to the default named session", () => {
        expect(herdrPageMatchesSnapshotSession("live", "default", "default")).toBe(true)
        expect(herdrPageMatchesSnapshotSession("live", "work", "default")).toBe(false)
        expect(herdrPageMatchesSnapshotSession("live", "live", "default")).toBe(false)
        expect(herdrPageMatchesSnapshotSession("work", "work", "default")).toBe(true)
        expect(herdrPageMatchesSnapshotSession("work", "default", "default")).toBe(false)
        expect(herdrPageMatchesSnapshotSession(undefined, "work", "default")).toBe(false)
    })

    it("uses normalized topology ownership, with raw counts only when topology is globally empty", () => {
        const withTopology = normalizeHerdrSnapshot({
            protocol: 19,
            version: "0.8.0",
            snapshot: {
                workspaces: [
                    { workspace_id: "ws-1", tab_count: 9 },
                    { workspace_id: "ws-2", tab_count: 9 }
                ],
                tabs: [
                    { tab_id: "tab-1", workspace_id: "ws-1" },
                    { tab_id: "tab-2", workspace_id: "ws-1" }
                ]
            }
        })
        expect(resolveSpaceTabCount(withTopology.spaces[0], withTopology.tabs)).toBe(2)
        expect(resolveSpaceTabCount(withTopology.spaces[1], withTopology.tabs)).toBe(0)

        const withoutTopology = normalizeHerdrSnapshot({
            protocol: 19,
            version: "0.8.0",
            snapshot: {
                workspaces: [{ workspace_id: "ws-1", tab_count: 4 }]
            }
        })
        expect(withoutTopology.tabs).toEqual([])
        expect(resolveSpaceTabCount(withoutTopology.spaces[0], withoutTopology.tabs)).toBe(4)
    })

    it("maps mixed drops onto the nearest same-Space Herdr insert index", () => {
        const projected = [
            file("/a"),
            herdr("h1", "ws-1"),
            file("/b"),
            herdr("h2", "ws-1")
        ]
        expect(herdrInsertIndexForProjectedDrop(projected, "h1", 3, "ws-1")).toBe(1)
        expect(herdrInsertIndexForProjectedDrop(projected, "h1", 1, "ws-1")).toBeNull()
    })

    it("uses runtime ownership for legacy Herdr pages without stored workspace identity", () => {
        const first = { ...herdr("h1", "ws-1"), herdrWorkspaceId: undefined }
        const second = { ...herdr("h2", "ws-1"), herdrWorkspaceId: undefined }
        const runtimeWorkspaceByTabId = new Map([
            ["h1", "ws-1"],
            ["h2", "ws-1"]
        ])

        expect(
            herdrInsertIndexForProjectedDrop(
                [first, second],
                "h1",
                1,
                "ws-1",
                runtimeWorkspaceByTabId
            )
        ).toBe(1)
    })

    it("hydrates only usable focused-Space tabs in snapshot order", () => {
        const snapshot = hydrationSnapshot({
            focusedTabId: "tab-2",
            tabs: [
                hydrationTab("tab-1", "ws-1", "term-1", { label: "One", order: 0 }),
                hydrationTab("tab-2", "ws-1", "term-2", { label: "Two", order: 1, focused: true }),
                hydrationTab("tab-3", "ws-1", "term-3", { label: "Three", order: 2, paneCount: 3 }),
                hydrationTab("tab-hidden", "ws-2", "term-hidden", { label: "Hidden" }),
                hydrationTab("tab-empty", "ws-1", null, { label: "No terminal" })
            ]
        })
        const result = hydrateFocusedSpaceHerdrPages(
            [{ tabs: [file("/a.ts")], activePath: "/a.ts" }],
            snapshot,
            "default",
            0
        )
        expect(result?.groups[0].tabs.map((tab) => tab.path)).toEqual([
            "/a.ts",
            herdrPagePath("default", "term-1"),
            herdrPagePath("default", "term-2"),
            herdrPagePath("default", "term-3")
        ])
        expect(result?.groups[0].activePath).toBe(herdrPagePath("default", "term-2"))
        expect(result?.groups[0].tabs.filter((tab) => tab.kind === "herdr-terminal")).toHaveLength(3)
        expect(result?.groups[0].tabs.some((tab) => tab.herdrTabId === "tab-hidden")).toBe(false)
    })
})

function hydrationTab(
    id: string,
    workspaceId: string,
    terminalId: string | null,
    extras: Partial<HerdrTabInfo> = {}
): HerdrTabInfo {
    return {
        id,
        label: extras.label ?? id,
        order: extras.order ?? 0,
        workspaceId,
        paneCount: extras.paneCount ?? 1,
        status: extras.status ?? "idle",
        active: extras.active ?? false,
        focused: extras.focused ?? false,
        terminalId,
        paneId: extras.paneId ?? null,
        sessionName: extras.sessionName ?? "default"
    }
}

function hydrationSnapshot(overrides: Partial<HerdrSnapshot>): HerdrSnapshot {
    return {
        herdrSessionId: "default",
        protocol: 19,
        version: "0.8.0",
        spaces: [],
        agents: [],
        tabs: [],
        terminals: [],
        focusedWorkspaceId: "ws-1",
        focusedTabId: "tab-1",
        raw: {},
        ...overrides
    }
}
