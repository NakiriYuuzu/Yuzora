import { beforeEach, expect, it, vi } from "vitest"
import script from "../../src-tauri/src/preview_interaction.js?raw"
import { formatElementContext, isElementContext, type ElementContext } from "./elementContext"

interface Interaction {
    start(): void
    stop(): void
    poll(bindings: unknown[]): { commands: string[]; selection: ElementContext | null; selecting: boolean }
}
const interaction = () => (window as unknown as { __yuzoraBrowser: Interaction }).__yuzoraBrowser
beforeEach(() => {
    window.eval(script)
    interaction().stop()
    interaction().poll([])
    document.body.innerHTML = ""
    if (!CSS.escape) CSS.escape = value => value.replace(/[^a-zA-Z0-9_-]/g, "_")
})

it("copies a selected element with styles while removing script handlers and form values", () => {
    document.body.innerHTML = '<section id="card"><h1>Title</h1><input value="secret" type="password"><textarea>private draft</textarea><button onclick="alert(1)">Edit</button></section>'
    const element = document.getElementById("card")!
    vi.spyOn(element, "getBoundingClientRect").mockReturnValue({ width: 240, height: 80 } as DOMRect)
    interaction().start()
    const clicked = new MouseEvent("click", { bubbles: true, cancelable: true, composed: true })
    element.dispatchEvent(clicked)
    const result = interaction().poll([])
    expect(result.selecting).toBe(false)
    expect(clicked.defaultPrevented).toBe(true)
    expect(isElementContext(result.selection)).toBe(true)
    expect(result.selection).toMatchObject({ selector: "#card", width: 240, height: 80 })
    const copied = formatElementContext(result.selection!, "/project/index.html")
    expect(copied).toContain("/project/index.html")
    expect(copied).not.toMatch(/secret|private draft|onclick/)
    expect(copied).toContain("Computed styles:")
})

it("selects open shadow DOM and same-origin iframe content, and cancels with Escape", () => {
    const host = document.body.appendChild(document.createElement("div"))
    host.id = "host"
    const child = host.attachShadow({ mode: "open" }).appendChild(document.createElement("button"))
    child.textContent = "Shadow"
    interaction().start()
    child.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true }))
    expect(interaction().poll([]).selection?.selector).toBe("#host >>> button")
    const frame = document.body.appendChild(document.createElement("iframe"))
    frame.id = "frame"
    frame.contentDocument!.body.innerHTML = '<button id="inside">Inside</button>'
    interaction().start()
    frame.contentDocument!.getElementById("inside")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, composed: true }))
    expect(interaction().poll([]).selection?.selector).toBe("#frame >> #inside")
    interaction().start()
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))
    expect(interaction().poll([]).selecting).toBe(false)
})

it("relays only configured tab shortcuts and leaves composition alone", () => {
    const binding = { id: "nextTab", key: "TAB", ctrl: true, meta: false, alt: false, shift: false }
    interaction().poll([binding])
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true, isComposing: true, bubbles: true }))
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true, bubbles: true, cancelable: true }))
    expect(interaction().poll([binding]).commands).toEqual(["nextTab"])
})

it("bounds very large selections and consumes each selection only once", () => {
    const element = document.body.appendChild(document.createElement("section"))
    element.textContent = "large ".repeat(20000)
    interaction().start()
    element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }))
    const selected = interaction().poll([]).selection!
    expect(selected.truncated).toBe(true)
    expect(selected.html.length).toBeLessThanOrEqual(24576)
    expect(formatElementContext(selected, "https://example.test").length).toBeLessThanOrEqual(32768)
    expect(interaction().poll([]).selection).toBeNull()
})
