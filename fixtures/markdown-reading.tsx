// Production editor with in-memory files. No native filesystem or Git actions.
import { useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import { RichMarkdownEditor } from "../src/editor/RichMarkdownEditor"
import { getView } from "../src/editor/viewRegistry"
import { Button } from "../src/components/ui/button"
import { files, ROOT } from "../src/demo/runtime"
import { useWorkspaceStore } from "../src/state/workspaceStore"
import i18n from "../src/lib/i18n"
import "../src/styles.css"
import "../src/theme/system-tone.css"

const names = ["安全 Markdown", "進階 Markdown", "大型 Markdown", "大型程式碼"]
const paragraph = "Continuous reading keeps the **complete document** available without section controls. Cross-document references use the same definitions and the original source remains unchanged. ".repeat(8)
const samples = [
  "# 可以直接編輯\n\n普通 Markdown 保留 **富文字** 編輯功能。\n",
  "# 直接閱讀\n\n<div><strong>進階 HTML 已直接渲染</strong></div>\n\n[跨段連結][shared]\n\n| 欄位 | 內容 |\n| --- | --- |\n| 顯示 | 完整表格 |\n\n[shared]: https://example.com/reading\n\n## 文件結尾\n\nADVANCED-END\n",
  "# 大型文件起點\n\n[跨文件參照][shared]\n\n" + Array.from({ length: 8500 }, (_, i) => `## Reading block ${i + 1}\n\n${paragraph}\n\n`).join("") + "[shared]: https://example.com/large\n\n## 完整文件結尾\n\nLARGE-DOCUMENT-END\n",
  "# 大型程式碼起點\n\n```text\n" + "complete-code-line 0123456789 abcdefghijklmnopqrstuvwxyz\n".repeat(240000) + "CODE-BLOCK-END\n```\n\n## 程式碼後方內容\n\nAFTER-CODE-END\n",
]
const paths = samples.map((content, index) => {
  const name = `markdown-reading-${index}.md`
  files[name] = content
  return `${ROOT}/${name}`
})
useWorkspaceStore.getState().setWorkspace(ROOT, "demo-workspace")
useWorkspaceStore.getState().openTab(paths[0], 0)

function Acceptance() {
  const [sample, setSample] = useState(0)
  const [unchanged, setUnchanged] = useState(true)
  useEffect(() => {
    let previous: unknown
    const id = setInterval(() => {
      const doc = getView(paths[sample])?.state.doc
      if (!doc || doc === previous) return
      previous = doc
      setUnchanged(doc.toString() === samples[sample])
    }, 300)
    return () => clearInterval(id)
  }, [sample])
  return <main style={{ height: "100vh", display: "flex", flexDirection: "column", background: "var(--paper-0)", color: "var(--ink-1)" }}>
    <div style={{ display: "flex", flexWrap: "wrap", gap: 8, alignItems: "center", padding: 12, borderBottom: "1px solid var(--line-1)" }}>
      {names.map((name, index) => <Button key={name} variant={sample === index ? "secondary" : "outline"} onClick={() => { useWorkspaceStore.getState().openTab(paths[index], 0); setSample(index); setUnchanged(true) }}>{name}</Button>)}
      <span role="status">{samples[sample].length.toLocaleString()} chars · {unchanged ? "原文未變" : "原文已修改"}</span>
    </div>
    <RichMarkdownEditor key={paths[sample]} path={paths[sample]} groupIndex={0} />
  </main>
}
void i18n.changeLanguage("zh-TW").then(() => createRoot(document.getElementById("root")!).render(<Acceptance />))
