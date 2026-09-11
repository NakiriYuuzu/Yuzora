// Read-only visual acceptance: production renderer, synthetic topology, no Git IPC.
import { useState } from "react"
import { createRoot } from "react-dom/client"
import { LogGraph } from "../src/workbench/git/LogGraph"
import type { LogCommit } from "../src/lib/types"
import { Button } from "../src/components/ui/button"
import i18n from "../src/lib/i18n"
import "../src/styles.css"
import "../src/theme/system-tone.css"

const BRANCHES = 32
const commit = (hash: string, parents: string[], subject: string, refs: LogCommit["refs"] = []): LogCommit => ({ hash, shortHash: hash.slice(0, 7), parents, subject, refs, authorName: "Graph QA", authorEmail: "graph@example.invalid", timestamp: 1_784_000_000 })
const history: LogCommit[] = [commit("merge-all", Array.from({ length: BRANCHES }, (_, i) => `branch-${i}-0`), "Merge 32 independent branches — graph topology acceptance", [{ kind: "local", name: "main" }])]
for (let row = 0; row < 12; row++) {
  for (let branch = 0; branch < BRANCHES; branch++) history.push(commit(
    `branch-${branch}-${row}`,
    [row === 11 ? "base" : `branch-${branch}-${row + 1}`],
    row === 0 && branch === 31 ? "第32條分支：" + "完整提交訊息與交錯分支應可水平捲動查看。".repeat(8) + "SUBJECT-END-32" : `Branch ${branch + 1} / commit ${row + 1}`,
    row === 0 ? [{ kind: "local", name: `feature/branch-${branch + 1}` }] : []
  ))
}
history.push(commit("base", [], "Shared base — all 32 lanes converge"))

function Acceptance() {
  const [selected, setSelected] = useState<string | null>(null)
  const [count, setCount] = useState(260)
  const [width, setWidth] = useState(680)
  const [loads, setLoads] = useState(0)
  return <main style={{ padding: 20, height: "100vh", display: "flex", flexDirection: "column", gap: 12, background: "var(--paper-0)", color: "var(--ink-1)" }}>
    <div style={{ display: "flex", gap: 12, alignItems: "center" }}><strong>32 branches · octopus merge · 386 commits</strong><Button variant="outline" onClick={() => setWidth(width === 680 ? 460 : 680)}>Toggle narrow</Button><span role="status">Selected: {selected ?? "none"} · Loads: {loads}</span></div>
    <section style={{ width, maxWidth: "100%", flex: 1, minHeight: 0, display: "flex", border: "1px solid var(--line-1)" }}>
      <LogGraph commits={history.slice(0, count)} selectedHash={selected} onSelect={setSelected} hasMore={count < history.length} loadingMore={false} onLoadMore={() => { setLoads(value => value + 1); setCount(history.length) }} />
    </section>
  </main>
}
void i18n.changeLanguage("zh-TW").then(() => createRoot(document.getElementById("root")!).render(<Acceptance />))
