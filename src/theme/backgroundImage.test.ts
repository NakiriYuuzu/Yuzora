import { afterEach, describe, expect, it, vi } from "vitest"

import { fitWithin, hasTransparency, loadBackgroundImage, saveBackgroundImage } from "./backgroundImage"

describe("background image sizing", () => {
  it("scales the long edge down to the limit and keeps the aspect ratio", () => {
    expect(fitWithin(5120, 2880)).toEqual({ width: 2560, height: 1440 })
    expect(fitWithin(3000, 6000, 1500)).toEqual({ width: 750, height: 1500 })
  })

  it("never upscales a small image", () => {
    expect(fitWithin(800, 600)).toEqual({ width: 800, height: 600 })
  })

  it("spots any transparent pixel so it can be kept as PNG", () => {
    expect(hasTransparency(new Uint8ClampedArray([10, 20, 30, 255, 40, 50, 60, 255]))).toBe(false)
    expect(hasTransparency(new Uint8ClampedArray([10, 20, 30, 255, 40, 50, 60, 0]))).toBe(true)
  })
})

/** Just enough IndexedDB to drive one transaction to complete or abort. */
function installIndexedDb(outcome: "complete" | "abort", stored?: unknown) {
  const transaction: Record<string, unknown> = { error: outcome === "abort" ? new DOMException("full", "QuotaExceededError") : null }
  const request = (result?: unknown) => {
    const req: Record<string, unknown> = { result }
    queueMicrotask(() => {
      (req.onsuccess as (() => void) | undefined)?.()
      queueMicrotask(() => (transaction[outcome === "complete" ? "oncomplete" : "onabort"] as () => void)())
    })
    return req
  }
  const store = { put: () => request(), get: () => request(stored), delete: () => request() }
  const db = { transaction: () => ({ ...transaction, objectStore: () => store, set oncomplete(f: () => void) { transaction.oncomplete = f }, set onabort(f: () => void) { transaction.onabort = f }, set onerror(f: () => void) { transaction.onerror = f } }), close: vi.fn() }
  vi.stubGlobal("indexedDB", {
    open: () => {
      const req: Record<string, unknown> = { result: db }
      queueMicrotask(() => (req.onsuccess as () => void)())
      return req
    },
  })
  return db
}

describe("background image storage", () => {
  afterEach(() => vi.unstubAllGlobals())

  it("reports a save that aborts at commit instead of claiming success", async () => {
    const db = installIndexedDb("abort")
    await expect(saveBackgroundImage(new Blob(["x"]))).rejects.toThrow("full")
    expect(db.close).toHaveBeenCalled()
  })

  it("resolves only once the transaction completes", async () => {
    installIndexedDb("complete")
    await expect(saveBackgroundImage(new Blob(["x"]))).resolves.toBeUndefined()
    const image = new Blob(["jpeg"], { type: "image/jpeg" })
    installIndexedDb("complete", image)
    await expect(loadBackgroundImage()).resolves.toBe(image)
    installIndexedDb("complete", "not a blob")
    await expect(loadBackgroundImage()).resolves.toBeNull()
  })
})
