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
    })

    localStorage.setItem(WORKBENCH_LAYOUT_STORAGE_KEY, "{not-json")
    expect(loadWorkbenchLayout()).toEqual({
      ...workbenchLayoutInitialState,
    })
  })

  it("ignores retired layout fields and retains the Markdown ratio", () => {
    localStorage.setItem(
      WORKBENCH_LAYOUT_STORAGE_KEY,
      JSON.stringify({
        version: 99,
        markdownEditorRatio: 0.62,
        markdownPreviewVisibility: { "/should-not-load.md": true },
        unknown: "ignored",
      }),
    )

    expect(loadWorkbenchLayout()).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.62,
    })
  })

  it("round-trips only the whitelisted layout fields through one storage key", () => {
    const store = useWorkbenchLayoutStore.getState()
    store.setMarkdownEditorRatio(0.6)

    expect(loadWorkbenchLayout()).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.6,
    })
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)!)).toEqual({
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: 0.6,
    })
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
  })

  it("keeps in-memory state usable when localStorage rejects a write", () => {
    localStorage.setItem = vi.fn(() => {
      throw new Error("quota exceeded")
    })

    expect(() => useWorkbenchLayoutStore.getState().setMarkdownEditorRatio(0.65)).not.toThrow()
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.65)
  })
})
