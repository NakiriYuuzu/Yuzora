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
