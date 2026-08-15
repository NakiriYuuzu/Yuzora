import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen, waitFor } from "@testing-library/react"

import type { GitConsoleEntry } from "../../state/gitStore"

// ConsoleTab only reads useGitStore.consoleLog and imports no IPC, but the
// store module pulls in @/lib/ipc at import time; stub it so nothing hits Tauri.
vi.mock("../../lib/ipc", () => ({
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" })),
    gitStatus: vi.fn(async () => ({})),
    gitBranches: vi.fn(async () => ({ local: [], remote: [], tags: [] })),
    gitRemoteProbe: vi.fn(async () => "no"),
    gitFetch: vi.fn(async () => undefined)
}))

const { ConsoleTab } = await import("./ConsoleTab")
const { useGitStore, initialGitState } = await import("../../state/gitStore")

function entry(over: Partial<GitConsoleEntry>): GitConsoleEntry {
    return { id: 1, cmd: "git fetch", out: [], tone: "ok", time: "14:01", ...over }
}

describe("ConsoleTab", () => {
    beforeEach(() => {
        useGitStore.setState(initialGitState)
    })
    afterEach(() => cleanup())

    it("shows the empty state when no ops have run", () => {
        render(<ConsoleTab />)
        expect(screen.getByText("No git commands run yet")).toBeInTheDocument()
    })

    it("renders an entry's cmd, time and output lines", () => {
        useGitStore.setState({
            consoleLog: [
                entry({
                    id: 7,
                    cmd: "git pull --rebase",
                    time: "14:05",
                    out: ["Successfully rebased.", "Fast-forwarded main."]
                })
            ]
        })
        render(<ConsoleTab />)
        expect(screen.getByText("$ git pull --rebase")).toBeInTheDocument()
        expect(screen.getByText("14:05")).toBeInTheDocument()
        expect(screen.getByText("Successfully rebased.")).toBeInTheDocument()
        expect(screen.getByText("Fast-forwarded main.")).toBeInTheDocument()
        // No empty state once there's at least one entry.
        expect(screen.queryByText("No git commands run yet")).not.toBeInTheDocument()
    })

    it("renders newest-first order as stored", () => {
        useGitStore.setState({
            consoleLog: [
                entry({ id: 2, cmd: "git push", time: "14:10" }),
                entry({ id: 1, cmd: "git fetch", time: "14:01" })
            ]
        })
        render(<ConsoleTab />)
        const cmds = screen.getAllByText(/^\$ git/).map((el) => el.textContent)
        expect(cmds).toEqual(["$ git push", "$ git fetch"])
    })

    it("uses both-axis ScrollArea so long unbreakable tokens remain reachable", async () => {
        const longToken = `git show ${"a".repeat(180)}`
        const observerCallbacks: ResizeObserverCallback[] = []
        class RecordingResizeObserver {
            private readonly callback: ResizeObserverCallback
            constructor(callback: ResizeObserverCallback) {
                this.callback = callback
                observerCallbacks.push(callback)
            }
            observe() {
                this.callback([], this as unknown as ResizeObserver)
            }
            unobserve() {}
            disconnect() {}
        }
        const previousRO = globalThis.ResizeObserver
        vi.stubGlobal("ResizeObserver", RecordingResizeObserver)

        try {
            useGitStore.setState({
                consoleLog: [
                    entry({
                        id: 9,
                        cmd: longToken,
                        out: ["ok"],
                        time: "15:00"
                    })
                ]
            })
            render(<ConsoleTab />)

            const viewport = document.querySelector(
                '[data-slot="scroll-area-viewport"]'
            ) as HTMLElement
            expect(viewport).toBeTruthy()
            expect(viewport.tabIndex).toBe(0)
            expect(viewport.contains(screen.getByText(`$ ${longToken}`))).toBe(true)

            // Radix type="auto" only mounts scrollbars after overflow is measured.
            Object.defineProperty(viewport, "offsetWidth", { configurable: true, value: 120 })
            Object.defineProperty(viewport, "scrollWidth", { configurable: true, value: 480 })
            Object.defineProperty(viewport, "offsetHeight", { configurable: true, value: 80 })
            Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: 320 })
            for (const callback of observerCallbacks) {
                callback([], {} as ResizeObserver)
            }

            await waitFor(() => {
                const root = document.querySelector('[data-slot="scroll-area"]') as HTMLElement
                expect(
                    root.querySelector(
                        '[data-slot="scroll-area-scrollbar"][data-orientation="vertical"]'
                    )
                ).toBeTruthy()
                expect(
                    root.querySelector(
                        '[data-slot="scroll-area-scrollbar"][data-orientation="horizontal"]'
                    )
                ).toBeTruthy()
            })
        } finally {
            vi.stubGlobal("ResizeObserver", previousRO)
        }
    })
})
