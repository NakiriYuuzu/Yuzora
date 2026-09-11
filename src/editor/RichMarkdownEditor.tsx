import { MarkdownDocumentPreview } from "@/workbench/MarkdownDocumentPreview"
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { EditorContent, useEditor, useEditorState } from "@tiptap/react"
import StarterKit from "@tiptap/starter-kit"
import { Markdown } from "@tiptap/markdown"
import { TableKit } from "@tiptap/extension-table"
import TaskList from "@tiptap/extension-task-list"
import TaskItem from "@tiptap/extension-task-item"
import { Compartment, StateEffect, type Text } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { undo, redo, undoDepth, redoDepth } from "@codemirror/commands"
import { Bold, Italic, Heading2, List, ListOrdered, Quote, Code, Undo2, Redo2 } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { ScrollArea } from "@/components/ui/scroll-area"
import { EditorPane } from "./EditorPane"
import { markdownRoundTripSafe, needsMarkdownSource } from "./markdownSafety"
import { useWorkspaceStore } from "@/state/workspaceStore"
import "./richMarkdown.css"

const RICH_DOCUMENT_LIMIT = 128 * 1024

// Preserve the document's leading/trailing newlines when Tiptap serializes edits.
function preserveDocumentWhitespace(original: string, replacement: string): string {
    return (original.match(/^\n*/)?.[0] ?? "") + replacement.replace(/^\n+|\n+$/g, "")
        + (original.match(/\n*$/)?.[0] ?? "")
}

interface DocumentOwner { view: EditorView; save: () => void; editable: boolean }

/** CodeMirror remains the document/undo/save authority in both modes. */
export function RichMarkdownEditor({ path, groupIndex }: { path: string; groupIndex: number }) {
    const { t } = useTranslation("richMarkdown")
    const [mode, setMode] = useState<"document" | "source">(() => useWorkspaceStore.getState().pendingReveal?.path === path ? "source" : "document")
    const [loadedOwner, setOwner] = useState<(DocumentOwner & { path: string; version: number }) | null>(null)
    const ownerVersion = useRef(0)
    const owner = loadedOwner?.path === path ? loadedOwner : null
    const ready = useCallback((view: EditorView, save: () => void, editable: boolean) => {
        setOwner({ view, save, editable, path, version: ++ownerVersion.current })
    }, [path])
    useEffect(() => useWorkspaceStore.subscribe((state) => {
        if (state.pendingReveal?.path === path) setMode("source")
    }), [path])
    return <section className="rich-markdown">
        <div className="rich-markdown-modebar">
            <span>Markdown</span>
            <ToggleGroup type="single" value={mode} onValueChange={(value) => { if (value === "document" || value === "source") setMode(value) }} aria-label={t("mode")}>
                <ToggleGroupItem value="document">{t("document")}</ToggleGroupItem>
                <ToggleGroupItem value="source">{t("source")}</ToggleGroupItem>
            </ToggleGroup>
        </div>
        <div className={mode === "source" || !owner ? "flex min-h-0 flex-1" : "hidden"} inert={mode !== "source" && !!owner}>
            <EditorPane path={path} groupIndex={groupIndex} onReady={ready} />
        </div>
        {owner && <div className={mode === "document" ? "flex min-h-0 flex-1 flex-col" : "hidden"} inert={mode !== "document"}>
            <MarkdownDocument key={owner.version} owner={owner} documentActive={mode === "document"} />
        </div>}
    </section>
}

interface DocumentModel { doc: Text; ready: boolean; revision: number; restoreFocus?: boolean }

function MarkdownDocument({ owner, documentActive }: { owner: DocumentOwner; documentActive: boolean }) {
    const [model, setModel] = useState<DocumentModel>(() => ({ doc: owner.view.state.doc, ready: true, revision: 0 }))
    const modelRef = useRef(model)
    const internalEdit = useRef(false)
    const [compartment] = useState(() => new Compartment())
    const changeModel = useCallback((next: DocumentModel) => { modelRef.current = next; setModel(next) }, [])
    useEffect(() => {
        let active = true
        let timer: ReturnType<typeof setTimeout> | undefined
        const refresh = (doc: Text, restoreFocus = false) => {
            if (!active || owner.view.state.doc !== doc) return
            changeModel({ doc, ready: true, revision: modelRef.current.revision + 1, restoreFocus })
        }
        if (documentActive && (!modelRef.current.ready || modelRef.current.doc !== owner.view.state.doc)) refresh(owner.view.state.doc)
        const listener = EditorView.updateListener.of((update) => {
            if (!active || !update.docChanged) return
            const current = modelRef.current
            const doc = update.state.doc
            if (internalEdit.current) {
                changeModel({ ...current, doc })
                return
            }
            clearTimeout(timer)
            const richFocused = !!document.activeElement?.closest(".rich-markdown-prose, .rich-markdown-toolbar")
            const restoreFocus = richFocused || (!current.ready && current.restoreFocus)
            // Unmount the stale rich editor immediately; source/undo updates own
            // the whole document and only the latest revision may become editable.
            changeModel({ ...current, doc, ready: false, restoreFocus })
            if (documentActive) timer = setTimeout(() => refresh(doc, restoreFocus), 150)
        })
        owner.view.dispatch({ effects: compartment.get(owner.view.state) === undefined
            ? StateEffect.appendConfig.of(compartment.of(listener))
            : compartment.reconfigure(listener) })
        return () => {
            active = false
            clearTimeout(timer)
            // EditorPane may destroy its sibling view first; destroy removes dom.
            if (owner.view.dom.parentNode) owner.view.dispatch({ effects: compartment.reconfigure([]) })
        }
    }, [owner, documentActive, changeModel, compartment])
    const source = useMemo(() => documentActive ? model.doc.toString() : "", [documentActive, model.doc])
    const richEligible = useMemo(() => documentActive && owner.editable
        && source.length <= RICH_DOCUMENT_LIMIT
        && new TextEncoder().encode(source).byteLength <= RICH_DOCUMENT_LIMIT
        && !needsMarkdownSource(source), [documentActive, owner.editable, source])
    const apply = (text: string) => {
        const current = modelRef.current
        if (!current.ready || !owner.editable || !richEligible || owner.view.state.doc !== current.doc) return
        const original = current.doc.toString()
        const insert = preserveDocumentWhitespace(original, text)
        if (insert === original) return
        internalEdit.current = true
        try {
            owner.view.dispatch({ changes: { from: 0, to: current.doc.length, insert }, userEvent: "input.richMarkdown" })
        } finally { internalEdit.current = false }
    }
    if (!documentActive) return null
    return model.ready && richEligible
        ? <MarkdownRichEditor key={model.revision} source={source} owner={owner} onChange={apply} restoreFocus={model.restoreFocus} />
        : <MarkdownDocumentPreview content={source} />
}

function MarkdownRichEditor({ source, owner, onChange, restoreFocus }: {
    source: string; owner: DocumentOwner; onChange: (text: string) => void; restoreFocus?: boolean
}) {
    const { t } = useTranslation("richMarkdown")
    const writeEnabled = useRef(false)
    const editor = useEditor({
        extensions: [StarterKit.configure({ undoRedo: false, link: { openOnClick: false, autolink: false } }), Markdown, TableKit, TaskList, TaskItem.configure({ nested: true, a11y: { checkboxLabel: (node) => t("taskCheckbox", { text: node.textContent }) } })],
        content: source,
        contentType: "markdown",
        autofocus: restoreFocus ? "start" : false,
        editorProps: {
            attributes: { class: "rich-markdown-prose", role: "textbox", "aria-label": t("editor"), "aria-multiline": "true" },
            handleKeyDown: (_view, event) => {
                if (!(event.metaKey || event.ctrlKey)) return false
                if (event.key.toLowerCase() === "s") { event.preventDefault(); owner.save(); return true }
                if (event.key.toLowerCase() === "z") { event.preventDefault(); (event.shiftKey ? redo : undo)(owner.view); return true }
                return false
            }
        },
        onUpdate: ({ editor: current }) => { if (writeEnabled.current) onChange(current.getMarkdown()) },
    }, [owner])
    // A document whose parser round-trip changes its meaning stays in reading mode.
    const safe = !!editor && markdownRoundTripSafe(source, editor.getMarkdown())
    useLayoutEffect(() => {
        writeEnabled.current = safe
        return () => { writeEnabled.current = false }
    }, [safe])
    const format = useEditorState({ editor, selector: ({ editor: current }) => ({
        bold: current?.isActive("bold"), italic: current?.isActive("italic"), heading: current?.isActive("heading", { level: 2 }),
        bullets: current?.isActive("bulletList"), ordered: current?.isActive("orderedList"), quote: current?.isActive("blockquote"), code: current?.isActive("codeBlock")
    }) })
    if (!editor) return null
    if (!safe) return <MarkdownDocumentPreview content={source} />
    const controls = [
        { id: "bold", Icon: Bold, run: () => editor.chain().focus().toggleBold().run() },
        { id: "italic", Icon: Italic, run: () => editor.chain().focus().toggleItalic().run() },
        { id: "heading", Icon: Heading2, run: () => editor.chain().focus().toggleHeading({ level: 2 }).run() },
        { id: "bullets", Icon: List, run: () => editor.chain().focus().toggleBulletList().run() },
        { id: "ordered", Icon: ListOrdered, run: () => editor.chain().focus().toggleOrderedList().run() },
        { id: "quote", Icon: Quote, run: () => editor.chain().focus().toggleBlockquote().run() },
        { id: "code", Icon: Code, run: () => editor.chain().focus().toggleCodeBlock().run() }
    ] as const
    return <>
        <div className="rich-markdown-toolbar" role="toolbar" aria-label={t("formatting")}>
            <ToggleGroup type="multiple" value={controls.filter((control) => format?.[control.id]).map((control) => control.id)} aria-label={t("formatting")}>
                {controls.map(({ id, Icon, run }) => <ToggleGroupItem key={id} value={id} aria-label={t(id)} title={t(id)} onClick={run}><Icon aria-hidden="true" /></ToggleGroupItem>)}
            </ToggleGroup>
            <Button variant="ghost" size="icon-sm" aria-label={t("undo")} disabled={!undoDepth(owner.view.state)} onClick={() => undo(owner.view)}><Undo2 aria-hidden="true" /></Button>
            <Button variant="ghost" size="icon-sm" aria-label={t("redo")} disabled={!redoDepth(owner.view.state)} onClick={() => redo(owner.view)}><Redo2 aria-hidden="true" /></Button>
        </div>
        <ScrollArea className="rich-markdown-scroll"><EditorContent editor={editor} /></ScrollArea>
    </>
}
