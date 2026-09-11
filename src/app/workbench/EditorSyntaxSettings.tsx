import { useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { buildExtensions } from "@/editor/cmExtensions"
import { SYNTAX_THEMES, useEditorSettingsStore, type SyntaxTheme } from "@/state/editorSettingsStore"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { SettingCard } from "./settingsPrimitives"

export function EditorSyntaxSettings() {
    const { t } = useTranslation("editorPreferences")
    const theme = useEditorSettingsStore(s => s.syntaxTheme)
    const setTheme = useEditorSettingsStore(s => s.setSyntaxTheme)
    const host = useRef<HTMLDivElement>(null)
    useEffect(() => {
        if (!host.current) return
        const view = new EditorView({ parent: host.current, state: EditorState.create({
            doc: '// Syntax colors follow the app appearance\ninterface Workspace { name: string; active: boolean }\nconst workspace: Workspace = { name: "Yuzora", active: true };\nfunction openWorkspace(id: number) { return workspace.name; }',
            extensions: buildExtensions("preview.ts", { readonly: true, syntaxOff: false }, () => {}, () => {}, false),
        }) })
        return () => view.destroy()
    }, [])
    return <>
        <SettingCard label={t("syntaxTheme")} sub={t("syntaxDescription")}>
            <Select value={theme} onValueChange={v => setTheme(v as SyntaxTheme)}>
                <SelectTrigger aria-label={t("syntaxTheme")}><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>{SYNTAX_THEMES.map(id => <SelectItem key={id} value={id}>{t(`themes.${id}`)}</SelectItem>)}</SelectGroup></SelectContent>
            </Select>
        </SettingCard>
        <div ref={host} className="editor-pane rounded-md border" aria-label={t("preview")} />
    </>
}
