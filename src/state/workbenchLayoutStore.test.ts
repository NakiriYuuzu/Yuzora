import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  WORKBENCH_LAYOUT_STORAGE_KEY,
  WORKBENCH_LAYOUT_VERSION,
  loadWorkbenchLayout,
  useWorkbenchLayoutStore,
  workbenchLayoutInitialState,
} from "./workbenchLayoutStore"

function installLocalStorage(): void {
  const values = new Map<string, string>()
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, String(value)),
    removeItem: (key: string) => void values.delete(key),
    clear: () => values.clear(),
    key: (index: number) => [...values.keys()][index] ?? null,
    get length() {
      return values.size
    },
  }

  Object.defineProperty(globalThis, "localStorage", {
    value: storage,
    configurable: true,
    writable: true,
  })
}

function resetStore(): void {
  useWorkbenchLayoutStore.setState({
    ...workbenchLayoutInitialState,
    terminalWorkspaceRatios: {},
  })
}

beforeEach(() => {
  installLocalStorage()
  resetStore()
})

describe("workbenchLayoutStore persistence", () => {
  it("uses versioned defaults when storage is missing or malformed", () => {
    expect(loadWorkbenchLayout()).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.5,
      terminalRatioScope: "global",
      terminalGlobalRatio: 0.3,
      terminalWorkspaceRatios: {},
    })

    localStorage.setItem(WORKBENCH_LAYOUT_STORAGE_KEY, "{not-json")
    expect(loadWorkbenchLayout()).toEqual({
      ...workbenchLayoutInitialState,
      terminalWorkspaceRatios: {},
    })
  })

  it("falls back invalid fields independently and filters the workspace map", () => {
    localStorage.setItem(
      WORKBENCH_LAYOUT_STORAGE_KEY,
      JSON.stringify({
        version: 99,
        markdownEditorRatio: 0.62,
        terminalRatioScope: "project",
        terminalGlobalRatio: -0.1,
        terminalWorkspaceRatios: {
          "/valid": 0.75,
          "/too-large": 1.1,
          "/not-finite": "0.4",
          "": 0.4,
        },
        markdownPreviewVisibility: { "/should-not-load.md": true },
        unknown: "ignored",
      }),
    )

    expect(loadWorkbenchLayout()).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.62,
      terminalRatioScope: "global",
      terminalGlobalRatio: 0.3,
      terminalWorkspaceRatios: { "/valid": 0.75 },
    })
  })

  it("round-trips only the whitelisted layout fields through one storage key", () => {
    const store = useWorkbenchLayoutStore.getState()
    store.setMarkdownEditorRatio(0.6)
    store.setTerminalRatio(null, 0.4)
    store.setTerminalRatioScope("workspace", "/workspace")
    store.setTerminalRatio("/workspace", 0.7)

    expect(loadWorkbenchLayout()).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.6,
      terminalRatioScope: "workspace",
      terminalGlobalRatio: 0.4,
      terminalWorkspaceRatios: { "/workspace": 0.7 },
    })
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)!)).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.6,
      terminalRatioScope: "workspace",
      terminalGlobalRatio: 0.4,
      terminalWorkspaceRatios: { "/workspace": 0.7 },
    })
  })

  it("hydrates persisted Terminal preferences without applying an effective layout clamp", () => {
    localStorage.setItem(
      WORKBENCH_LAYOUT_STORAGE_KEY,
      JSON.stringify({
        version: WORKBENCH_LAYOUT_VERSION,
        markdownEditorRatio: 0.5,
        terminalRatioScope: "workspace",
        terminalGlobalRatio: 0.95,
        terminalWorkspaceRatios: { "/one": 0.27 },
      })
    )

    useWorkbenchLayoutStore.setState(loadWorkbenchLayout())

    const hydrated = useWorkbenchLayoutStore.getState()
    expect(hydrated.terminalGlobalRatio).toBe(0.95)
    expect(hydrated.effectiveTerminalRatio("/one")).toBe(0.27)
    expect(hydrated.effectiveTerminalRatio("/two")).toBe(0.95)
  })

  it("accepts inclusive ratio boundaries and ignores invalid action inputs", () => {
    const store = useWorkbenchLayoutStore.getState()

    store.setMarkdownEditorRatio(0)
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0)
    store.setMarkdownEditorRatio(1)
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(1)

    store.setMarkdownEditorRatio(-0.01)
    store.setMarkdownEditorRatio(1.01)
    store.setMarkdownEditorRatio(Number.NaN)
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(1)

    store.setTerminalRatio(null, 0.8)
    store.setTerminalRatio(null, Number.POSITIVE_INFINITY)
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.8)
  })

  it("keeps in-memory state usable when localStorage rejects a write", () => {
    localStorage.setItem = vi.fn(() => {
      throw new Error("quota exceeded")
    })

    expect(() => useWorkbenchLayoutStore.getState().setMarkdownEditorRatio(0.65)).not.toThrow()
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.65)
  })
})

describe("terminal ratio scope", () => {
  it("uses global ratios by default and workspace overrides only in workspace scope", () => {
    const store = useWorkbenchLayoutStore.getState()
    store.setTerminalRatio(null, 0.45)

    expect(store.effectiveTerminalRatio("/one")).toBe(0.45)
    store.setTerminalRatioScope("workspace", "/one")
    useWorkbenchLayoutStore.getState().setTerminalRatio("/one", 0.7)

    expect(useWorkbenchLayoutStore.getState().effectiveTerminalRatio("/one")).toBe(0.7)
    expect(useWorkbenchLayoutStore.getState().effectiveTerminalRatio("/two")).toBe(0.45)
  })

  it("seeds both scope transitions from the current effective ratio without a jump", () => {
    const store = useWorkbenchLayoutStore.getState()
    store.setTerminalRatio(null, 0.4)

    const beforeWorkspace = store.effectiveTerminalRatio("/one")
    store.setTerminalRatioScope("workspace", "/one")
    expect(useWorkbenchLayoutStore.getState().effectiveTerminalRatio("/one")).toBe(beforeWorkspace)
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios).toEqual({ "/one": 0.4 })

    useWorkbenchLayoutStore.getState().setTerminalRatio("/one", 0.72)
    const beforeGlobal = useWorkbenchLayoutStore.getState().effectiveTerminalRatio("/one")
    useWorkbenchLayoutStore.getState().setTerminalRatioScope("global", "/one")

    expect(useWorkbenchLayoutStore.getState().effectiveTerminalRatio("/one")).toBe(beforeGlobal)
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.72)
  })

  it("changes only the scope when no canonical workspace is available", () => {
    const store = useWorkbenchLayoutStore.getState()
    store.setTerminalRatio(null, 0.48)
    store.setTerminalRatioScope("workspace", null)

    const state = useWorkbenchLayoutStore.getState()
    expect(state.terminalRatioScope).toBe("workspace")
    expect(state.terminalWorkspaceRatios).toEqual({})
    expect(state.effectiveTerminalRatio("/later-workspace")).toBe(0.48)
  })
})
