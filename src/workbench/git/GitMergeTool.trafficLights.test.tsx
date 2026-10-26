import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({ lights: false }))
vi.mock("@/lib/platform", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/platform")>()),
    showsNativeTrafficLights: () => platform.lights
}))
vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    gitConflictSides: vi.fn(() => new Promise(() => undefined))
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))

import { useGitConflictStore } from "@/state/gitConflictStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { GitMergeTool } from "./GitMergeTool"

beforeEach(() => {
    useGitStore.setState({ ...initialGitState, environment: { status: "ready", root: "/w", version: "2.50.1" } })
    useGitConflictStore.setState({ mergePath: "f.txt" })
})
afterEach(() => {
    cleanup()
    useGitConflictStore.setState({ conflictsOpen: false, mergePath: null })
})

describe("GitMergeTool title bar inset", () => {
    for (const lights of [true, false]) {
        it(`marks the header native-lights=${lights}`, async () => {
            platform.lights = lights
            render(<GitMergeTool />)
            const title = await screen.findByText(/f\.txt/, { selector: "[data-slot='dialog-title']" })
            const header = title.closest("[data-slot='dialog-header']")!
            expect(header.getAttribute("data-native-lights")).toBe(String(lights))
            expect(header.className).toContain("data-[native-lights=true]:pl-")
        })
    }
})
