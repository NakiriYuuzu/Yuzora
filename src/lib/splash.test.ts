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

    it("加上淡出 class，fallback timer 後移除節點並清掉 html inline background", () => {
        vi.useFakeTimers()
        const el = insertSplash()

        dismissSplash()

        expect(el.classList.contains("yz-splash-leave")).toBe(true)
        expect(document.getElementById("yz-splash")).not.toBeNull()

        vi.runAllTimers()

        expect(document.getElementById("yz-splash")).toBeNull()
        expect(document.documentElement.style.backgroundColor).toBe("")
    })

    it("transitionend 先到時同樣移除，且 timer 補刀不重複移除", () => {
        vi.useFakeTimers()
        const el = insertSplash()

        dismissSplash()
        el.dispatchEvent(new Event("transitionend"))

        expect(document.getElementById("yz-splash")).toBeNull()
        expect(() => vi.runAllTimers()).not.toThrow()
    })

    it("幂等：退場進行中重複呼叫不重啟流程", () => {
        vi.useFakeTimers()
        insertSplash()

        dismissSplash()
        expect(() => dismissSplash()).not.toThrow()

        vi.runAllTimers()
        expect(document.getElementById("yz-splash")).toBeNull()
    })

    it("測試環境被拆掉後，fallback timer 不得再去碰全域 document", () => {
        vi.useFakeTimers()
        insertSplash()

        dismissSplash()

        // 重現 CI 上的 splash flake：vitest 跑完一個測試檔就會拆掉 jsdom 環境，
        // 但 dismissSplash 排的 fallback timer（FADE_MS + 150）沒有取消路徑、
        // 而且 jsdom 永遠不觸發 transitionend，所以它一定會在之後觸發。屆時全域
        // `document` 已不存在，裸參照會拋 ReferenceError；vitest 把它記成
        // Unhandled Error 並讓整個 run exit 1——即使每一個測試都通過。
        const realDocument = globalThis.document
        Reflect.deleteProperty(globalThis, "document")
        try {
            expect(() => vi.runAllTimers()).not.toThrow()
        } finally {
            ;(globalThis as { document?: Document }).document = realDocument
        }
    })

    it("prefers-reduced-motion 時跳過動畫立即移除", () => {
        const matchMediaSpy = vi.spyOn(window, "matchMedia").mockImplementation(
            (query: string) =>
                ({
                    matches: query.includes("prefers-reduced-motion"),
                    media: query,
                    onchange: null,
                    addListener: () => {},
                    removeListener: () => {},
                    addEventListener: () => {},
                    removeEventListener: () => {},
                    dispatchEvent: () => false
                }) as unknown as MediaQueryList
        )
        try {
            insertSplash()

            dismissSplash()

            expect(document.getElementById("yz-splash")).toBeNull()
            expect(document.documentElement.style.backgroundColor).toBe("")
        } finally {
            matchMediaSpy.mockRestore()
        }
    })
})
