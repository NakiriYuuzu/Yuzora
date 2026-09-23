// Production editor extensions, no filesystem or Git writes.
import { useEffect, useRef, useState } from "react"
import { createRoot } from "react-dom/client"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { buildExtensions } from "../src/editor/cmExtensions"
import { conflictMarkers } from "../src/editor/conflictMarkers"
import { Button } from "../src/components/ui/button"
import "../src/styles.css"
import "../src/editor/editor.css"
import "../src/lib/i18n"

function SearchAcceptance() {
    const host = useRef<HTMLDivElement>(null)
    const [large, setLarge] = useState(false)
    const [minimap, setMinimap] = useState(true)
    useEffect(() => {
        const text = Array.from({ length: large ? 100000 : 1000 }, (_, i) => `const value${i} = "Find text ${i}";`).join("\n")
        const view = new EditorView({
            state: EditorState.create({ doc: text, extensions: [...buildExtensions("/search.ts", { readonly: false, syntaxOff: large }, () => {}, () => {}, minimap), conflictMarkers()] }),
            parent: host.current!,
        })
        view.focus()
        return () => view.destroy()
    }, [large, minimap])
    return <main style={{ height: "100dvh", display: "flex", flexDirection: "column", padding: 16, gap: 12, background: "var(--background)", color: "var(--foreground)" }}>
        <div style={{ display: "flex", gap: 12 }}>
            <Button variant="outline" onClick={() => setLarge(value => !value)}>{large ? "100,000 lines" : "1,000 lines"}</Button>
            <Button variant="outline" onClick={() => setMinimap(value => !value)}>Minimap {minimap ? "on" : "off"}</Button>
            <Button variant="outline" onClick={() => document.documentElement.classList.toggle("dark")}>Theme</Button>
        </div>
        <div className="editor-pane" ref={host} style={{ minHeight: 0, flex: 1 }} />
    </main>
}
const root = createRoot(document.getElementById("root")!)
root.render(<SearchAcceptance />)
if (import.meta.hot) import.meta.hot.dispose(() => root.unmount())
