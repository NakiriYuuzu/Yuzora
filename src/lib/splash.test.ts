import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { dismissSplash } from "./splash"

function insertSplash(): HTMLElement {
    const el = document.createElement("div")
    el.id = "yz-splash"
    document.body.appendChild(el)
    return el
}

beforeEach(() => {
    document.documentElement.style.backgroundColor = "#fbfaf6"
})

afterEach(() => {
    document.getElementById("yz-splash")?.remove()
    document.documentElement.style.removeProperty("background-color")
    vi.useRealTimers()
    vi.restoreAllMocks()
})

describe("dismissSplash", () => {
    it("無 splash 節點時安靜 no-op（HMR／test 環境）", () => {
        expect(() => dismissSplash()).not.toThrow()
        expect(document.getElementById("yz-splash")).toBeNull()
    })

    it("同步移除節點並清掉 html inline background，不依賴 timer 或 transition", () => {
        vi.useFakeTimers()
        insertSplash()
        const timeoutSpy = vi.spyOn(window, "setTimeout")

        dismissSplash()

        expect(document.getElementById("yz-splash")).toBeNull()
        expect(document.documentElement.style.backgroundColor).toBe("")
        expect(timeoutSpy).not.toHaveBeenCalled()
    })

    it("幂等：重複呼叫保持安靜", () => {
        insertSplash()

        dismissSplash()

        expect(() => dismissSplash()).not.toThrow()
        expect(document.getElementById("yz-splash")).toBeNull()
    })
})
