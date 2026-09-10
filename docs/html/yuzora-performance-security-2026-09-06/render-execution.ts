import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const e = (v: unknown) => String(v).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!))
const unique = (xs: string[]): string[] => [...new Set(xs)]
const read = (dir: string, name: string) => JSON.parse(readFileSync(resolve(dir, name), 'utf8'))
const join = (xs: string[]) => xs.length ? xs.join('、') : '—'
const mode = (t: any) => t.kind === 'gate' ? 'gate' : t.requires.length === 0 ? 'direct' : t.mockAfter.length ? 'contract' : 'dependent'
const modeLabel: Record<string, string> = { direct: '可獨立開始', contract: '契約後可平行', dependent: '交付有前置', gate: '聯合驗收' }
const lane = (id: string) => id.startsWith('DB-') ? 'DB' : id.startsWith('SH-') ? 'SSH/SFTP' : /^(RT-|LS-|LG-)/.test(id) ? 'Runtime' : /^(FE-|SEARCH-|SEC-)/.test(id) ? 'Frontend/Search' : 'Integration'

export function buildExecutionPlan(dir: string, observations: any[]) {
  const strategy = read(dir, 'execution-strategy.json')
  const parts = ['database', 'runtime', 'frontend'].map(name => read(dir, `execution-${name}.json`))
  const work = [...strategy.tasks, ...parts.flatMap(p => p.tasks)].map(t => ({ ...t, kind: 'work', status: 'planned', lane: lane(t.id) }))
  const gates = strategy.gates.map(g => ({ ...g, kind: 'gate', status: 'planned', lane: 'Integration', owner: '整合／驗收負責人', files: [], mockAfter: [],
    requires: unique(['INT', ...work.filter(t => t.auditIds.some((id: string) => g.auditIds.includes(id)) || t.featureIds.some((id: string) => g.featureIds.includes(id))).map(t => t.id)]),
    deliverable: '使用真正實作取代 mock，完成跨層回歸與故障注入。', note: '多個 requires 是 AND gate；前置各包可依自己的契約平行開發。' }))
  const final = { id: 'G-ALL', title: '完整回歸、量測與交付', kind: 'gate', status: 'planned', lane: 'Integration', auditIds: [], featureIds: [], owner: '整合／驗收負責人', files: [], mockAfter: [],
    requires: [...work, ...gates].map(t => t.id), deliverable: '55 項與四個新能力完整處置、before/after 證據、跨平台回歸與更新報告。',
    gate: '完整 frontend／Rust／三引擎 integration 與真 Tauri 驗收；50 次 lifecycle 循環、2 小時混合負載；量測候選需有明確採用或保留原實作的數據理由，不宣稱未實測改善。',
    note: '重型量測獨占主機。此 gate 不包含 Git 寫入、PR、發布或使用真實遠端帳密。' }
  const tasks = [...work, ...gates, final]
  const taskMap = new Map(tasks.map(t => [t.id, t]))
  if (taskMap.size !== tasks.length) throw new Error('Duplicate execution task IDs')
  const visiting = new Set<string>(), visited = new Set<string>()
  const visit = (id: string) => {
    if (!taskMap.has(id)) throw new Error(`Unknown task ${id}`)
    if (visiting.has(id)) throw new Error(`Dependency cycle at ${id}`)
    if (visited.has(id)) return
    visiting.add(id)
    const t = taskMap.get(id)!
    for (const dep of unique([...t.requires, ...t.mockAfter])) visit(dep)
    visiting.delete(id); visited.add(id)
  }
  tasks.forEach(t => visit(t.id))
  const auditSet = new Set(observations.map(x => x.id)), featureSet = new Set(strategy.features.map((x: any) => x.id))
  for (const t of tasks) {
    for (const id of t.auditIds) if (!auditSet.has(id)) throw new Error(`Unknown audit ${id}`)
    for (const id of t.featureIds) if (!featureSet.has(id)) throw new Error(`Unknown feature ${id}`)
  }
  const auditCoverage = Object.fromEntries(observations.map(x => [x.id, work.filter(t => t.auditIds.includes(x.id)).map(t => t.id)]))
  const featureCoverage = Object.fromEntries(strategy.features.map((x: any) => [x.id, work.filter(t => t.featureIds.includes(x.id)).map(t => t.id)]))
  for (const [id, ids] of Object.entries({ ...auditCoverage, ...featureCoverage })) if (!(ids as string[]).length) throw new Error(`Unmapped scope ${id}`)

  // Global integration locks take precedence; all contributing task references are retained.
  const locks = [...strategy.locks, ...parts.flatMap(p => p.locks)]
  const allFiles = unique([...locks.flatMap(l => l.files), ...work.flatMap(t => t.files)])
  const ownership = allFiles.map(path => {
    const rules = locks.filter(l => l.files.includes(path))
    const touching = unique([...work.filter(t => t.files.includes(path)).map(t => t.id), ...rules.flatMap(l => l.tasks)])
    const owners = unique(work.filter(t => t.files.includes(path)).map(t => t.owner))
    const owner = rules[0]?.owner ?? (owners.length === 1 ? owners[0] : 'UNRESOLVED')
    if (owner === 'UNRESOLVED') throw new Error(`Unresolved ownership ${path}`)
    for (const id of touching) if (!taskMap.has(id)) throw new Error(`Unknown lock task ${id}`)
    return { path, owner, tasks: touching, reason: rules[0]?.reason ?? '單一工作包直接負責；其他包只透過既定介面使用。', existsAtAssessment: existsSync(resolve(dir, '../../..', path)) }
  })
  const plan = { ...strategy, tasks, gates: undefined, locks: undefined, ownership, auditCoverage, featureCoverage,
    counts: { auditItems: observations.length, features: strategy.features.length, workPackages: work.length, integrationGates: gates.length + 1, totalTasks: tasks.length, ownedPaths: ownership.length },
    generatedFrom: ['execution-strategy.json', 'execution-database.json', 'execution-runtime.json', 'execution-frontend.json', 'observations.json'] }
  writeFileSync(resolve(dir, 'execution-plan.json'), JSON.stringify(plan, null, 2) + '\n')
  writeFileSync(resolve(dir, 'EXECUTION-PLAN.md'), renderMarkdown(plan))
  return plan
}

function renderMarkdown(p: any) {
  const lines = [`# ${p.title}`, '', `評估日期：${p.date}。狀態：**${p.status}**。來源 HEAD：\`${p.sourceCommit}\`。`, '', p.scope, '',
    `共 ${p.counts.workPackages} 個工程工作包（含契約／整合職責）與 ${p.counts.integrationGates} 個聯合驗收 gate；涵蓋 ${p.counts.auditItems}/55 原項及 ${p.counts.features}/4 新能力。所有 task 都是 planned。`, '',
    '機器可讀版本：[execution-plan.json](execution-plan.json)。主報告：[HTML](../yuzora-performance-security-2026-09-06.html#execution)。本檔由 render-report.ts 產生；修改 execution-strategy/database/runtime/frontend.json 後重新產生，避免手改衍生檔漂移。', '',
    '## 啟動控制與指定模型', '',
    `目前：**${p.executionControl.state}／待命**。${p.executionControl.startTrigger}`, '',
    `開發：最多 ${p.executionControl.maxConcurrentDevelopmentAgents} 個 \`${p.executionControl.developmentAgent.model}\`／\`${p.executionControl.developmentAgent.reasoning_effort}\` worker。最終 review：\`${p.executionControl.finalReviewAgent.model}\`／\`${p.executionControl.finalReviewAgent.reasoning_effort}\`。`, '',
    p.executionControl.coordinator, '',
    '### 收到指令後的派工前置', '', ...p.executionControl.preflight.map((s: string) => `- ${s}`), '', ...p.executionControl.scheduling.map((s: string) => `- ${s}`), '',
    '### 最終 review 啟動條件（全部滿足）', '', ...p.executionControl.reviewStartConditions.map((s: string) => `- ${s}`), '', p.executionControl.reviewScope, '', p.executionControl.reviewFindings, '', p.executionControl.permissions, '',
    '以下是未執行的 spawn 參數模板；task_name 與完整 bounded 工作訊息須在實際派工時提供。', '',
    '```json', JSON.stringify({ development: p.executionControl.developmentAgent, finalReview: p.executionControl.finalReviewAgent }, null, 2), '```', '',
    '## 如何閱讀相依關係', '', ...p.definitions.map((d: any) => `- **${d.term}**：${d.text}`), '',
    '## 已接受的行為與預設', '', ...p.decisions.map((d: string) => `- ${d}`), '', '| 項目 | 初始預設（待同條件量測） |', '|---|---|', ...p.defaults.map((d: any) => `| ${d.area} | ${d.value} |`), '',
    '## 本輪發現的進行中變更與交接', '', `唯讀快照：${p.workspaceAlignment.assessedAt}。${p.workspaceAlignment.basis}`, '', p.workspaceAlignment.summary, '', p.workspaceAlignment.rule, '',
    `受影響工作包：${join(p.workspaceAlignment.affectedTasks)}。`, '', ...p.workspaceAlignment.paths.map((s: string) => `- \`${s}\``), '',
    '## 分工與派工', '', ...p.dispatchPolicy.map((s: string) => `- ${s}`), '', ...p.parallelExamples.flatMap((x: any) => [`### ${x.title}`, '', x.work, '', x.condition, '']),
    '## 工作包與交付 DAG', '', 'requires 是正式交付前置；mockAfter 是可提前開發的契約門檻。表內存在前置不代表下游必須空等；也不代表共享檔案可以多人同寫。', '',
    '| 工作包 | 範圍 | 方式 | 正式交付前置（AND） | 可先 mock 的契約 |', '|---|---|---|---|---|',
    ...p.tasks.map((t: any) => `| ${t.id} ${t.title} | ${join([...t.auditIds, ...t.featureIds])} | ${modeLabel[mode(t)]} | ${join(t.requires)} | ${join(t.mockAfter)} |`), '',
    '### 必须依序完成的重點', '',
    '- DB-A 增量 accounting → DB-R page/pin/snapshot；DB-B 控制／背壓 → heartbeat timeout 啟用與 DB-BT 正式量測；DB-R＋DB-B＋DB-T → DB-Q 壓力處置啟用。',
    '- SH-C → SH-A 真 adapter → SH-B 真 transfer/promotion；SH-F 與 SH-B 可以在 SH-C 後先 mock 平行，最後才共同通過 G-SSH。',
    '- LG-C → writer ownership → rotation/retention → cursor 跨清理驗收；同 logging.rs 由一位 writer 完成，cursor fixture 可先行。',
    '- Search quota/cancel/settlement → 平行 workers 正式啟用；tree generation 防晚到回填 → cache eviction 啟用。',
    '- LS-C → LSP backend 與 frontend 平行 → G-LSP；production chunks／worker／CSS 與 CSP 各自可開發，最後 AND 驗收 G-ASSET，不互相建立循環相依。',
    '- 各 service bounded stop → INT 全 app shutdown 整合 → 故障注入；log 最後 drain，不能為縮短工期跳過退出 gate。', '',
    '## 每包交付與驗收', '', ...p.tasks.flatMap((t: any) => [`### ${t.id} — ${t.title}`, '', `- 負責角色：${t.owner}；狀態：planned。`, `- 交付：${t.deliverable}`, `- Gate：${t.gate}`, `- 相依：${join(t.requires)}；先 mock：${join(t.mockAfter)}。`, `- 責任範圍：${t.files.length ? t.files.map((f: string) => `\`${f}\``).join('、') : '契約／驗收文件；不宣稱取得產品共享檔案寫入權。'}`, `- 協作注意：${t.note}`, '']),
    '## 共享檔案交接後的目標 writer', '', '下表是計畫的最終 writer 指派，優先於子系統片段內的角色名稱；交接前仍維持現有工作者 ownership。task.files 是參與範圍，不是允許每個參與者直接修改。INT 集中持有全域檔案；domain owner 提交介面需求／adapter，由指定 writer 接線。新檔明確標示為預計新增。', '',
    '| 路徑 | 有效 writer | 相關包 | 理由 |', '|---|---|---|---|',
    ...p.ownership.map((x: any) => `| \`${x.path}\`${x.existsAtAssessment ? '' : '（預計新增／尚不存在）'} | ${x.owner} | ${join(x.tasks)} | ${x.reason} |`), '',
    '## Runtime 平行與序列化', '', '| 子系統 | 可受限平行 | 必須保留的序列化／原子性 |', '|---|---|---|', ...p.runtime.map((r: any) => `| ${r.work} | ${r.parallel} | ${r.serial} |`), '',
    '## 測試與量測排程', '', ...p.testScheduling.map((r: string) => `- ${r}`), '',
    '## 完整範圍對照', '', '同一 finding 可以映射多個交付包，代表各部分都需驗收，不是重複計算改善數。單靠契約包完成不能關閉原 finding。', '',
    '| 原稽核／新能力 | 工作包 |', '|---|---|', ...Object.entries({ ...p.auditCoverage, ...p.featureCoverage }).map(([id, ts]) => `| ${id} | ${join(ts as string[])} |`), '',
    '## 證據與界限', '', ...p.evidence.map((r: string) => `- ${r}`), '']
  return lines.join('\n')
}

export function renderExecutionHtml(p: any) {
  const links = (ids: string[]) => ids.length ? ids.map(id => `<a href="#task-${e(id)}">${e(id)}</a>`).join(' · ') : '無其他工作包前置'
  const rows = (xs: any[], f: (x: any) => string) => xs.map(f).join('')
  const table = (head: string[], body: string) => `<div class="table-wrap"><table><thead><tr>${head.map(s => `<th>${e(s)}</th>`).join('')}</tr></thead><tbody>${body}</tbody></table></div>`
  const list = (xs: string[]) => `<ul>${xs.map(s => `<li>${e(s)}</li>`).join('')}</ul>`
  return `<section id="execution"><h2>平行開發評估與已接受的實作計畫</h2>
  <p class="lede">${p.counts.workPackages} 個工程工作包 ＋ ${p.counts.integrationGates} 個聯合驗收 gate。涵蓋全部 55 項改善與 4 個多 database／schema 能力標籤。</p>
  <div class="callout"><strong>${e(p.status)}</strong>。所有工作包標示 planned；本次只更新文件。開發可平行、檔案 writer 互斥、runtime 序列化是三件不同的事。預設一位整合者＋三位實作 worker；沒有工時資料，不估算完成日期或加速倍數。</div>
  <div id="standby" class="panel" style="margin-top:16px;border-left:4px solid var(--accent)"><h3>待命：等待明確開始指令</h3><p>開發模型 <strong>${e(p.executionControl.developmentAgent.model)} / ${e(p.executionControl.developmentAgent.reasoning_effort)}</strong>，同時最多 ${p.executionControl.maxConcurrentDevelopmentAgents} 名 worker；全部工作與驗收完成後，才 spawn <strong>${e(p.executionControl.finalReviewAgent.model)} / ${e(p.executionControl.finalReviewAgent.reasoning_effort)}</strong> 做唯讀最終 review。</p><p>${e(p.executionControl.startTrigger)}</p><details><summary>啟動前檢查與最終 review 門檻</summary>${list(p.executionControl.preflight)}${list(p.executionControl.reviewStartConditions)}<p>${e(p.executionControl.reviewScope)}</p><p>${e(p.executionControl.reviewFindings)}</p></details></div>
  <div class="panel" style="margin-top:16px;border-left:4px solid var(--amber)"><h3>派工前先對齊現有 host／identity 工作</h3><p>${e(p.workspaceAlignment.summary)}</p><p>${e(p.workspaceAlignment.rule)}</p><p>對齊工作包：${links(['ALIGN-HOST'])}。受影響：${links(p.workspaceAlignment.affectedTasks)}。</p><p class="meta">唯讀快照 ${e(p.workspaceAlignment.assessedAt)}；原稽核程式碼引用依固定 commit 驗證，沒有覆寫其他工作者的變更。</p></div>
  <p style="margin-top:14px"><a href="yuzora-performance-security-2026-09-06/EXECUTION-PLAN.md">完整交接文件</a> · <a href="yuzora-performance-security-2026-09-06/execution-plan.json">完整 DAG／ownership JSON</a></p>
  <div class="grid2">${rows(p.parallelExamples, x => `<div class="panel"><h3>${e(x.title)}</h3><p>${e(x.work)}</p><p class="muted">${e(x.condition)}</p></div>`)}</div>
  <details class="panel" style="margin-top:16px"><summary>已接受的功能行為與資源預設</summary>${list(p.decisions)}${table(['項目', '初始預設（待量測）'], rows(p.defaults, x => `<tr><td>${e(x.area)}</td><td>${e(x.value)}</td></tr>`))}</details>
  </section>
  <section id="roadmap"><h2>哪些必須先完成</h2><div class="flow">
  <div><b>DB · AND GATES</b><p>accounting → page/pin<br>控制／背壓 ＋ 狀態 → admission<br>backend ＋ 兩條 UI → 多 DB 驗收</p></div>
  <div><b>SSH · CONTRACT FIRST</b><p>契約 → adapter／backend mock／UI mock 可並行<br>真 adapter → 真 transfer → 端到端驗收</p></div>
  <div><b>LSP · TWO SIDES</b><p>accepted-version／credit 契約<br>→ writer ＋ frontend 可並行<br>→ 一致性、取消與退出驗收</p></div>
  <div><b>ASSETS · NO CYCLE</b><p>lazy chunks／Markdown worker／CSP／CSS 可並行<br>→ production asset 聯合 gate</p></div></div>
  <p class="legend">箭頭是交付前置，＋表示所有前置均需完成。完整關係見下方每包 requires；logging writer→rotation→cursor、tree generation→eviction、search quota→parallel enable 等同包內順序也必須保留。</p>
  ${table(['概念', '排程規則'], rows(p.definitions, x => `<tr><td>${e(x.term)}</td><td>${e(x.text)}</td></tr>`))}
  <h3 style="margin-top:22px">逐包派工與驗收</h3><p>以功能或開發方式篩選，展開查看責任範圍與 gate。每個參與者的檔案清單不等於直接写入權，有效 writer 以後面的 ownership 表為準。</p>
  <div class="controls" style="position:static"><input id="task-search" type="search" aria-label="搜尋工程工作包" placeholder="搜尋工作包、原項 ID、檔案或驗收…"><select id="task-lane" aria-label="依工程工作線篩選"><option value="">全部工作線</option>${unique(p.tasks.map((t: any) => t.lane)).map(s => `<option>${e(s)}</option>`).join('')}</select><select id="task-mode" aria-label="依開發方式篩選"><option value="">全部方式</option>${Object.entries(modeLabel).map(([v, s]) => `<option value="${v}">${s}</option>`).join('')}</select><span id="task-count" aria-live="polite">${p.tasks.length} / ${p.tasks.length}</span></div>
  <div id="task-list">${rows(p.tasks, t => `<details id="task-${e(t.id)}" class="plan-task panel" data-lane="${e(t.lane)}" data-mode="${mode(t)}" data-search="${e(JSON.stringify(t))}" style="margin-top:8px"><summary><strong>${e(t.id)} · ${e(t.title)}</strong> <span class="tag">${modeLabel[mode(t)]} · planned</span></summary><p style="margin-top:14px"><strong>範圍：</strong>${e(join([...t.auditIds, ...t.featureIds]))}　<strong>負責角色：</strong>${e(t.owner)}</p><p><strong>正式交付前置（AND）：</strong>${links(t.requires)}</p><p><strong>可先 mock 的契約：</strong>${t.mockAfter.length ? links(t.mockAfter) : '無；可先做不依賴實作的 fixture／設計準備。'}</p><p><strong>交付：</strong>${e(t.deliverable)}</p><p><strong>驗收 gate：</strong>${e(t.gate)}</p><p><strong>協作注意：</strong>${e(t.note)}</p><p class="meta">參與範圍：${t.files.length ? t.files.map(e).join(' · ') : '契約／整合／驗收職責'}</p></details>`)}</div><p id="task-empty" hidden>沒有符合條件的工作包。</p>
  </section>
  <section id="ownership"><h2>共用檔案：指定 writer，安排串行接線</h2><p>不同包可先開發獨立模組、adapter、reducer、component 與 fixture。全域檔案由 INT 集中接線；下表是交接後目標分工，優先於子系統片段內的角色名稱。交接前仍維持現有 writer，不覆寫進行中的工作。</p>
  <details class="panel"><summary>展開 ${p.ownership.length} 條有效路徑 ownership（含預計新增）</summary>${table(['檔案', '有效 writer', '工作包／原因'], rows(p.ownership, x => `<tr><td style="white-space:normal"><code>${e(x.path)}</code>${x.existsAtAssessment ? '' : '<br><small>預計新增／尚不存在</small>'}</td><td>${e(x.owner)}</td><td>${links(x.tasks)}<p>${e(x.reason)}</p></td></tr>`))}</details>
  ${list(p.dispatchPolicy)}</section>
  <section id="execution-runtime"><h2>Runtime 哪些可以平行、哪些仍須有序</h2>${table(['工作', '可受限平行', '序列化／原子性'], rows(p.runtime, x => `<tr><td>${e(x.work)}</td><td>${e(x.parallel)}</td><td>${e(x.serial)}</td></tr>`))}</section>
  <section id="execution-tests"><h2>測試也要排程，避免污染效能結果</h2>${list(p.testScheduling)}<p class="callout">來源與限制：${e(p.evidence.join(' '))}</p></section>
  <script>
  (()=>{const records=[...document.querySelectorAll('.plan-task')],search=document.getElementById('task-search'),lane=document.getElementById('task-lane'),mode=document.getElementById('task-mode');
  const filter=()=>{let n=0;const q=search.value.trim().toLocaleLowerCase();for(const r of records){r.hidden=Boolean((q&&!r.dataset.search.toLocaleLowerCase().includes(q))||(lane.value&&r.dataset.lane!==lane.value)||(mode.value&&r.dataset.mode!==mode.value));if(!r.hidden)n++;}document.getElementById('task-count').textContent=n+' / '+records.length;document.getElementById('task-empty').hidden=n!==0;};
  for(const el of [search,lane,mode])el.addEventListener('input',filter);
  const openHash=()=>{const el=document.getElementById(decodeURIComponent(location.hash.slice(1)));if(el&&el.classList.contains('plan-task')){search.value='';lane.value='';mode.value='';filter();el.open=true;}};window.addEventListener('hashchange',openHash);openHash();
  let saved=[];window.addEventListener('beforeprint',()=>{saved=records.map(r=>[r.open,r.hidden]);records.forEach(r=>{r.open=true;r.hidden=false;});});window.addEventListener('afterprint',()=>records.forEach((r,i)=>{r.open=saved[i][0];r.hidden=saved[i][1];}));
  })();
  </script>`
}
