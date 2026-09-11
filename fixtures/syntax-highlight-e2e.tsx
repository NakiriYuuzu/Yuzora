import { useState } from "react"
import { createRoot } from "react-dom/client"
import { EditorPane } from "../src/editor/EditorPane"
import { Button } from "../src/components/ui/button"
import { ScrollArea } from "../src/components/ui/scroll-area"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../src/components/ui/select"
import { files, ROOT } from "../src/demo/runtime"
import { useWorkspaceStore } from "../src/state/workspaceStore"
import { useEditorSettingsStore, type SyntaxTheme } from "../src/state/editorSettingsStore"
import { syntaxSamples, syntaxCoverage, type RenderedRole } from "./syntax-highlight-corpus"
import i18n from "../src/lib/i18n"
import "../src/styles.css"
import "../src/theme/system-tone.css"

const roleLabels: Record<RenderedRole, string> = { keyword: "關鍵字", string: "字串", comment: "註解", number: "數值", function: "函式", type: "型別", property: "屬性", tag: "標籤", heading: "標題", link: "連結", variable: "識別字", meta: "指示詞", definition: "一般定義", plain: "一般文字" }
const paths = syntaxSamples.map(sample => sample.files.map(filename => {
    const key = `syntax-${sample.id}/${filename}`
    files[key] = sample.code
    return `${ROOT}/${key}`
}))
useWorkspaceStore.getState().setWorkspace(ROOT, "demo-workspace")
useWorkspaceStore.getState().openTab(paths[0][0], 0)

function Acceptance() {
    const [index, setIndex] = useState(0)
    const [alias, setAlias] = useState(0)
    const [dark, setDark] = useState(document.documentElement.classList.contains("dark"))
    const theme = useEditorSettingsStore(state => state.syntaxTheme)
    const sample = syntaxSamples[index]
    const select = (next: number, nextAlias = 0) => {
        useWorkspaceStore.getState().openTab(paths[next][nextAlias], 0)
        setIndex(next)
        setAlias(nextAlias)
    }
    return <main style={{ height: "100vh", display: "flex", flexDirection: "column", background: "var(--paper-0)", color: "var(--ink-1)" }}>
        <header style={{ display: "flex", gap: 12, alignItems: "center", padding: "12px 16px", borderBottom: "1px solid var(--line-1)", flexWrap: "wrap" }}>
            <h1 style={{ fontSize: 18, fontWeight: 600 }}>語法高亮 · 29 種語言</h1>
            <span style={{ marginLeft: "auto" }}>配色</span>
            <Select value={theme} onValueChange={value => useEditorSettingsStore.getState().setSyntaxTheme(value as SyntaxTheme)}>
                <SelectTrigger aria-label="語法配色" className="w-32"><SelectValue /></SelectTrigger>
                <SelectContent>{["github", "yuzora", "one"].map(value => <SelectItem key={value} value={value}>{value === "github" ? "GitHub" : value === "one" ? "One" : "Yuzora"}</SelectItem>)}</SelectContent>
            </Select>
            <Button variant="outline" onClick={() => { document.documentElement.classList.toggle("dark", !dark); setDark(!dark) }}>{dark ? "切換淺色" : "切換深色"}</Button>
        </header>
        <div style={{ display: "flex", minHeight: 0, flex: 1 }}>
            <ScrollArea className="w-48 shrink-0 border-r" aria-label="語言清單">
                <nav style={{ display: "flex", flexDirection: "column", gap: 3, padding: 8 }}>
                    {syntaxSamples.map((item, i) => <Button key={item.id} variant={i === index ? "secondary" : "ghost"} aria-pressed={i === index} className="justify-start" onClick={() => select(i)}>{item.name}</Button>)}
                </nav>
            </ScrollArea>
            <section data-testid="syntax-sample" data-language={sample.id} data-filename={sample.files[alias]} style={{ display: "flex", flexDirection: "column", minWidth: 0, minHeight: 0, flex: 1 }}>
                <div style={{ padding: "12px 18px", borderBottom: "1px solid var(--line-1)" }}>
                    <div style={{ display: "flex", gap: 12, alignItems: "center", flexWrap: "wrap" }}>
                        <h2 style={{ fontSize: 20, fontWeight: 600 }}>{sample.name}</h2>
                        <Select value={String(alias)} onValueChange={value => select(index, Number(value))}>
                            <SelectTrigger aria-label="測試副檔名" className="w-44"><SelectValue /></SelectTrigger>
                            <SelectContent>{sample.files.map((file, i) => <SelectItem key={file} value={String(i)}>{file}</SelectItem>)}</SelectContent>
                        </Select>
                        <span>{index + 1} / {syntaxSamples.length}</span>
                        <span role="status">{syntaxCoverage[index].status === "partial" ? "部分語法或角色尚有限制" : "範例角色皆有對應配色"}</span>
                    </div>
                    {sample.note && <p style={{ marginTop: 8, fontSize: 12, color: "var(--ink-2)" }}>{sample.note}</p>}
                    <p style={{ marginTop: 8, fontSize: 12, color: "var(--ink-2)" }}>檢查項目：{sample.probes.map((probe, probeIndex) => <span key={`${probe.role}-${probe.text}`} data-syntax-probe={probe.text} data-expected-role={probe.renderedAs ?? probe.role} data-requested-role={probe.role}>
                        {probeIndex > 0 ? " · " : ""}{roleLabels[probe.role]} {probe.text}{probe.renderedAs ? `（${roleLabels[probe.renderedAs]}配色）` : ""}
                    </span>)}</p>
                </div>
                <EditorPane key={paths[index][alias]} path={paths[index][alias]} groupIndex={0} />
            </section>
        </div>
    </main>
}

void i18n.changeLanguage("zh-TW").then(() => createRoot(document.getElementById("root")!).render(<Acceptance />))
