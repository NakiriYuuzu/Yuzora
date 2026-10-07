import { copyTextInBackground } from "@/lib/clipboardFeedback"
import "./markdownCodeCopy.css"

// Lucide "copy"; a constant, never derived from document content.
const COPY_ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>'

/** A trusted control placed beside rendered code; it copies text, never markup. */
export function createCodeCopyButton(label: string, read: () => string): HTMLButtonElement {
    const button = document.createElement("button")
    button.type = "button"
    button.className = "markdown-code-copy"
    button.title = label
    button.setAttribute("aria-label", label)
    button.contentEditable = "false"
    button.innerHTML = COPY_ICON
    // Keep focus and any editor selection where they are.
    button.addEventListener("mousedown", event => event.preventDefault())
    button.addEventListener("click", event => {
        event.preventDefault()
        event.stopPropagation()
        copyTextInBackground(read())
    })
    return button
}

/** Adds a copy control to each rendered code block; returns their removal. */
export function attachCodeCopyButtons(root: HTMLElement, label: string): () => void {
    const buttons = Array.from(root.querySelectorAll("pre"), pre => {
        const code = pre.querySelector("code") ?? pre
        // A fence renders with its closing newline; copy the block's lines only.
        const button = createCodeCopyButton(label, () => (code.textContent ?? "").replace(/\n$/, ""))
        pre.append(button)
        return button
    })
    return () => { for (const button of buttons) button.remove() }
}
