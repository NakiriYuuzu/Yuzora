import { MarkdownPreview } from "@/workbench/MarkdownPreview"
import { useCallback, useEffect, useState } from "react"
import { EditorContent, useEditor, useEditorState } from "@tiptap/react"
import StarterKit from "@tiptap/starter-kit"
import { Markdown } from "@tiptap/markdown"
import { TableKit } from "@tiptap/extension-table"
import TaskList from "@tiptap/extension-task-list"
import TaskItem from "@tiptap/extension-task-item"
import { Compartment, StateEffect } from "@codemirror/state"
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

interface DocumentOwner { view: EditorView; save: () => void; editable: boolean }

/** CodeMirror remains the document/undo/save authority in both modes. */
export function RichMarkdownEditor({ path, groupIndex }: { path: string; groupIndex: number }) {
    const { t } = useTranslation("richMarkdown")
    const [mode, setMode] = useState<"document" | "source">(() => useWorkspaceStore.getState().pendingReveal?.path === path ? "source" : "document")
    const [owner, setOwner] = useState<DocumentOwner | null>(null)
    const ready = useCallback((view: EditorView, save: () => void, editable: boolean) => {
        setOwner({ view, save, editable })
    }, [])
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
            <MarkdownDocument path={path} owner={owner} onSource={() => setMode("source")} />
        </div>}
    </section>
}

function MarkdownDocument({ path, owner, onSource }: { path: string; owner: DocumentOwner; onSource: () => void }) {
    const { t } = useTranslation("richMarkdown")
    const [source, setSource] = useState(() => owner.view.state.doc.toString())
    const editor = useEditor({
        extensions: [StarterKit.configure({ undoRedo: false, link: { openOnClick: false, autolink: false } }), Markdown, TableKit, TaskList, TaskItem.configure({ nested: true, a11y: { checkboxLabel: (node) => t("taskCheckbox", { text: node.textContent }) } })],
        content: source,
        contentType: "markdown",
        editorProps: {
            attributes: { class: "rich-markdown-prose", role: "textbox", "aria-label": t("editor"), "aria-multiline": "true" },
            handleKeyDown: (_view, event) => {
                if (!(event.metaKey || event.ctrlKey)) return false
                if (event.key.toLowerCase() === "s") { event.preventDefault(); owner.save(); return true }
                if (event.key.toLowerCase() === "z") { event.preventDefault(); (event.shiftKey ? redo : undo)(owner.view); return true }
                return false
            }
        },
        onUpdate: ({ editor: current }) => {
            const text = current.getMarkdown()
            if (text === owner.view.state.doc.toString()) return
            owner.view.dispatch({ changes: { from: 0, to: owner.view.state.doc.length, insert: text }, userEvent: "input.richMarkdown" })
        }
    }, [owner])
    const safe = !!editor && owner.editable && !needsMarkdownSource(source) && markdownRoundTripSafe(source, editor.getMarkdown())
    useEffect(() => {
        const compartment = new Compartment()
        let active = true
        // The listener also sees source edits, undo and formatting.
        const listener = EditorView.updateListener.of((update) => {
            if (!active || !update.docChanged) return
            const text = update.state.doc.toString()
            setSource(text)
            if (editor && editor.getMarkdown() !== text) editor.commands.setContent(text, { contentType: "markdown", emitUpdate: false })
        })
        owner.view.dispatch({ effects: StateEffect.appendConfig.of(compartment.of(listener)) })
        return () => { active = false }
    }, [owner, editor])
    const format = useEditorState({ editor, selector: ({ editor: current }) => ({
        bold: current?.isActive("bold"), italic: current?.isActive("italic"), heading: current?.isActive("heading", { level: 2 }),
        bullets: current?.isActive("bulletList"), ordered: current?.isActive("orderedList"), quote: current?.isActive("blockquote"), code: current?.isActive("codeBlock")
    }) })
    if (!editor) return null
    if (!safe) return <><div className="rich-markdown-reading-note"><p>{t("advanced")}</p><Button variant="outline" size="sm" onClick={onSource}>{t("source")}</Button></div><MarkdownPreview sourcePath={path} embedded /></>
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
