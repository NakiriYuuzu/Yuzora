import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

import { installTerminalImeHandling } from "./terminalImeHandling"

const nextTask = () => new Promise((resolve) => window.setTimeout(resolve, 0))

describe("Windows IME input", () => {
    let terminal: Terminal | undefined
    let handling: { dispose: () => void } | undefined

    afterEach(() => {
        handling?.dispose()
        handling = undefined
        terminal?.dispose()
        terminal = undefined
        document.body.replaceChildren()
    })

    it("emits a Microsoft Pinyin commit when composition starts at offset zero", async () => {
        const container = document.createElement("div")
        Object.defineProperties(container, {
            clientWidth: { configurable: true, value: 800 },
            clientHeight: { configurable: true, value: 480 }
        })
        document.body.append(container)

        terminal = new Terminal({ cols: 80, rows: 24 })
        const emitted: string[] = []
        terminal.open(container)
        handling = installTerminalImeHandling(terminal, (data) => emitted.push(data))

        const textarea = terminal.textarea
        expect(textarea).not.toBeNull()
        if (!textarea) return

        textarea.value = ""
        textarea.selectionStart = 0
        textarea.selectionEnd = 0
        textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }))
        textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "ni" }))
        textarea.value = "ni"
        textarea.selectionStart = 2
        textarea.selectionEnd = 2
        await nextTask()

        textarea.dispatchEvent(new CompositionEvent("compositionend", { data: "你是" }))
        textarea.value = "你是"
        textarea.selectionStart = 2
        textarea.selectionEnd = 2
        await nextTask()

        expect(emitted).toEqual(["你是"])
    })

    it("emits the complete Microsoft Pinyin commit after TSF replaces the textarea value", async () => {
        const container = document.createElement("div")
        Object.defineProperties(container, {
            clientWidth: { configurable: true, value: 800 },
            clientHeight: { configurable: true, value: 480 }
        })
        document.body.append(container)

        terminal = new Terminal({ cols: 80, rows: 24 })
        const emitted: string[] = []
        terminal.open(container)
        handling = installTerminalImeHandling(terminal, (data) => emitted.push(data))

        const textarea = terminal.textarea
        expect(textarea).not.toBeNull()
        if (!textarea) return

        // Three characters already sent to the PTY remain in xterm's helper
        // textarea when Microsoft Pinyin begins composition.
        textarea.value = "   "
        textarea.selectionStart = 3
        textarea.selectionEnd = 3
        textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }))

        textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "n" }))
        textarea.value = "   n"
        textarea.selectionStart = 4
        textarea.selectionEnd = 4
        await nextTask()

        // Windows TSF selects and replaces the whole helper value from the
        // second frame onward, invalidating xterm's original start offset.
        textarea.selectionStart = 0
        textarea.selectionEnd = 4
        textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "ni" }))
        textarea.value = "ni"
        textarea.selectionStart = 2
        textarea.selectionEnd = 2
        await nextTask()

        textarea.dispatchEvent(new CompositionEvent("compositionend", { data: "你是" }))
        textarea.value = "你是"
        textarea.selectionStart = 2
        textarea.selectionEnd = 2
        await nextTask()
        await nextTask()

        expect(emitted).toEqual(["你是"])
    })
})

// Recorded from WKWebView with Apple Pinyin/Zhuyin direct insertion: the IME
// inserts before the keyCode 229 keydown, and fast typing presses the next key
// before the previous keyup.
describe("macOS IME direct insertion", () => {
    let terminal: Terminal | undefined
    let handling: { dispose: () => void } | undefined

    afterEach(() => {
        handling?.dispose()
        handling = undefined
        terminal?.dispose()
        terminal = undefined
        document.body.replaceChildren()
    })

    function open(): { textarea: HTMLTextAreaElement; emitted: string[] } {
        const container = document.createElement("div")
        Object.defineProperties(container, {
            clientWidth: { configurable: true, value: 800 },
            clientHeight: { configurable: true, value: 480 }
        })
        document.body.append(container)
        terminal = new Terminal({ cols: 80, rows: 24 })
        const emitted: string[] = []
        terminal.open(container)
        handling = installTerminalImeHandling(terminal, (data) => emitted.push(data))
        const textarea = terminal.textarea
        if (!textarea) throw new Error("xterm textarea missing")
        return { textarea, emitted }
    }

    function insertText(textarea: HTMLTextAreaElement, data: string) {
        textarea.dispatchEvent(new InputEvent("beforeinput", { data, inputType: "insertText", composed: true, cancelable: true }))
        textarea.value += data
        textarea.selectionStart = textarea.value.length
        textarea.selectionEnd = textarea.value.length
        textarea.dispatchEvent(new InputEvent("input", { data, inputType: "insertText", composed: true }))
    }

    function key(textarea: HTMLTextAreaElement, type: "keydown" | "keyup", key: string, code: string, keyCode: number) {
        textarea.dispatchEvent(new KeyboardEvent(type, { key, code, keyCode, bubbles: true, cancelable: true }))
    }

    it("keeps a punctuation mark typed before the previous key is released", async () => {
        const { textarea, emitted } = open()

        insertText(textarea, "你")
        key(textarea, "keydown", "你", "KeyN", 229)
        await nextTask()

        // "，" arrives while the previous key is still held down.
        insertText(textarea, "，")
        key(textarea, "keydown", "，", "Comma", 229)
        await nextTask()
        key(textarea, "keyup", "n", "KeyN", 78)
        key(textarea, "keyup", "，", "Comma", 188)
        await nextTask()

        expect(emitted).toEqual(["你", "，"])
    })

    it("keeps every character of a fast rollover run without duplicates", async () => {
        const { textarea, emitted } = open()

        insertText(textarea, "1")
        key(textarea, "keydown", "1", "Digit1", 229)
        await nextTask()
        insertText(textarea, "2")
        key(textarea, "keydown", "2", "Digit2", 229)
        await nextTask()
        key(textarea, "keyup", "1", "Digit1", 49)
        insertText(textarea, "3")
        key(textarea, "keydown", "3", "Digit3", 229)
        await nextTask()
        key(textarea, "keyup", "2", "Digit2", 50)
        key(textarea, "keyup", "3", "Digit3", 51)
        await nextTask()

        expect(emitted.join("")).toBe("123")
    })

    it("does not duplicate an insertion whose 229 keydown arrives first", async () => {
        const { textarea, emitted } = open()

        key(textarea, "keydown", "。", "Period", 229)
        insertText(textarea, "。")
        await nextTask()
        await nextTask()
        key(textarea, "keyup", "。", "Period", 190)

        expect(emitted).toEqual(["。"])
    })

    it.each([
        [" ", "Space", 32],
        ["A", "KeyA", 65]
    ])("does not repeat %j that xterm already sent from keypress", async (char, code, keyCode) => {
        // xterm sends space and A–Z from keypress without preventDefault, so
        // WKWebView still delivers the matching insertText afterwards.
        const { textarea, emitted } = open()
        const charCode = char.charCodeAt(0)
        key(textarea, "keydown", char, code, keyCode)
        textarea.dispatchEvent(new KeyboardEvent("keypress", { key: char, code, keyCode: charCode, charCode, bubbles: true, cancelable: true }))
        insertText(textarea, char)
        key(textarea, "keyup", char, code, keyCode)
        await nextTask()
        await nextTask()

        expect(emitted).toEqual([char])
    })

    it("does not duplicate a character typed right after a composition commit", async () => {
        const { textarea, emitted } = open()

        textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }))
        textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "ni" }))
        textarea.value = "ni"
        key(textarea, "keydown", "i", "KeyI", 229)
        textarea.value = "你"
        textarea.dispatchEvent(new CompositionEvent("compositionend", { data: "你" }))
        // The next key is still held from the composition, so xterm ignores the input.
        insertText(textarea, "，")
        key(textarea, "keydown", "，", "Comma", 229)
        await nextTask()
        await nextTask()
        await nextTask()

        expect(emitted.join("")).toBe("你，")
    })
})
