// Node is used because Bun's VM proxy compatibility prevents jsdom script evaluation.
import { readFileSync, existsSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { JSDOM } from 'jsdom'
import { execFileSync } from 'node:child_process'
const dir = dirname(fileURLToPath(import.meta.url))
const root = resolve(dir, '../../..')
const data = JSON.parse(readFileSync(resolve(dir, 'observations.json'), 'utf8'))
const plan = JSON.parse(readFileSync(resolve(dir, 'execution-plan.json'), 'utf8'))
const errors = []
const check = (ok, msg) => { if (!ok) errors.push(msg) }
const sourceSnapshot = new Map()
for (const i of data) for (const r of i.refs) {
  // Audit links point to an immutable commit. Other workers may move live source files.
  if (!sourceSnapshot.has(r.path)) {
    try {
      sourceSnapshot.set(r.path, execFileSync('git', ['show', `${plan.sourceCommit}:${r.path}`], { cwd: root, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 }))
    } catch {
      check(false, 'Missing source at audit commit ' + r.path)
      sourceSnapshot.set(r.path, '')
    }
  }
  check(r.line > 0 && r.line <= sourceSnapshot.get(r.path).split('\n').length, 'Invalid audit-commit line ' + r.path + ':' + r.line)
}
const html = readFileSync(resolve(dir, '../yuzora-performance-security-2026-09-06.html'), 'utf8')
const dom = new JSDOM(html, { runScripts: 'dangerously', pretendToBeVisual: true, url: 'https://audit.invalid/report.html' })
const d = dom.window.document
check(d.querySelectorAll('.finding').length === 55, '55 rendered items')
check(new Set([...d.querySelectorAll('[id]')].map(x => x.id)).size === d.querySelectorAll('[id]').length, 'Unique IDs')
const visible = () => [...d.querySelectorAll('.finding')].filter(x => !x.hidden)
const set = (id, value) => { const el = d.getElementById(id); el.value = value; el.dispatchEvent(new dom.window.Event('input')) }
set('priority', 'P1'); check(visible().length === 11, 'P1 count')
set('priority', 'P2'); check(visible().length === 38, 'P2 count')
set('priority', 'P3'); check(visible().length === 6, 'P3 count')
set('priority', ''); set('category', 'SSH／SFTP'); check(visible().length === 12, 'SSH filter')
set('category', ''); set('search', 'TLS'); check(visible().some(x => x.id === 'SEC01'), 'TLS search')
d.getElementById('expand-all').click(); check(visible().every(x => x.open), 'Expand visible')
d.getElementById('collapse-all').click(); check([...d.querySelectorAll('.finding')].every(x => !x.open), 'Collapse all')
set('search', 'audit-no-such-value-79283'); check(visible().length === 0 && !d.getElementById('empty').hidden, 'No results state')
set('search', ''); check(visible().length === 55, 'Restore all')
const tasks = new Map(plan.tasks.map(t => [t.id, t]))
check(tasks.size === plan.tasks.length, 'Unique execution task IDs')
check(plan.tasks.every(t => t.status === 'planned'), 'Do not claim product implementation complete')
check(plan.executionControl.state === 'standby', 'Development remains on standby')
check(plan.executionControl.developmentAgent.model === 'gpt-5.6-sol' && plan.executionControl.developmentAgent.reasoning_effort === 'medium', 'Requested development model/effort')
check(plan.executionControl.finalReviewAgent.model === 'gpt-6-astra' && plan.executionControl.finalReviewAgent.reasoning_effort === 'xhigh', 'Requested final review model/effort')
check(plan.executionControl.maxConcurrentDevelopmentAgents === 3 && plan.executionControl.reviewStartConditions.length >= 3, 'Concurrency and final-review gates recorded')
check(!!d.getElementById('standby'), 'Standby configuration rendered')
check(plan.counts.auditItems === 55 && Object.keys(plan.auditCoverage).length === 55, 'All 55 audit items mapped')
check(Object.keys(plan.featureCoverage).length === 4, 'All 4 new DB capabilities mapped')
for (const [id, owners] of Object.entries({ ...plan.auditCoverage, ...plan.featureCoverage })) {
  check(owners.length > 0 && owners.every(t => tasks.has(t)), 'Scope coverage '+id)
  check(owners.some(t => !/契約/.test(tasks.get(t).title)), 'Scope has implementation beyond a contract '+id)
}
const visiting = new Set(), visited = new Set()
function visit(id) {
  if (!tasks.has(id)) { check(false, 'Missing task dependency '+id); return }
  if (visiting.has(id)) { check(false, 'Execution DAG cycle '+id); return }
  if (visited.has(id)) return
  visiting.add(id)
  const t=tasks.get(id)
  for (const dep of new Set([...t.requires, ...t.mockAfter])) visit(dep)
  visiting.delete(id); visited.add(id)
}
plan.tasks.forEach(t => visit(t.id))
check(new Set(plan.ownership.map(x=>x.path)).size === plan.ownership.length, 'One effective writer per path')
check(plan.ownership.every(x=>x.owner && x.owner!=='UNRESOLVED' && x.tasks.every(t=>tasks.has(t))), 'Resolved ownership references')
const renderedTasks=()=>[...d.querySelectorAll('.plan-task')]
const visibleTasks=()=>renderedTasks().filter(x=>!x.hidden)
check(renderedTasks().length===plan.tasks.length, 'All execution tasks rendered')
for (const t of plan.tasks) check(!!d.getElementById('task-'+t.id), 'Task anchor '+t.id)
set('task-lane','DB'); check(visibleTasks().length===plan.tasks.filter(t=>t.lane==='DB').length, 'DB lane filter')
set('task-lane',''); set('task-mode','gate'); check(visibleTasks().length===plan.counts.integrationGates, 'Integration gate filter')
set('task-mode',''); set('task-search','DBM04'); check(visibleTasks().some(x=>x.id==='task-DB-UR'), 'New DB Preview capability search')
set('task-search','execution-no-such-value-951'); check(!visibleTasks().length&&!d.getElementById('task-empty').hidden, 'Execution empty state')
dom.window.location.hash='#task-DB-S'
dom.window.dispatchEvent(new dom.window.HashChangeEvent('hashchange'))
check(d.getElementById('task-DB-S').open&&!d.getElementById('task-DB-S').hidden, 'Task deep link opens and resets filters')
check(visibleTasks().length===plan.tasks.length && visible().length===55, 'Independent audit and task filters')
for (const a of d.querySelectorAll('a[href]')) {
  const href = a.getAttribute('href')
  if (href.startsWith('#')) check(!!d.getElementById(href.slice(1)), 'Anchor ' + href)
  else if (!href.startsWith('http')) check(existsSync(resolve(dir, '..', href)), 'Attachment ' + href)
}
const result = {
  date: '2026-09-06', entries: data.length,
  sourceFiles: new Set(data.flatMap(i => i.refs.map(r => r.path))).size,
  priorityCounts: Object.fromEntries(['P1', 'P2', 'P3'].map(p => [p, data.filter(i => i.priority === p).length])),
  offlineDomChecks: errors.length ? 'FAIL' : 'PASS: audit-commit source paths/line bounds, ids, filters, search, empty state, expand/collapse, attachments, anchors',
  sourceRevision: plan.sourceCommit,
  errors,
  qaRuntime: 'Node + jsdom; Bun VM proxy limitation required Node for offline script execution.',
  visualCheck: 'Not performed: browser URL policy blocked local file preview. No browser workaround attempted.',
  tests: { frontend: 60, rustSearch: 9, rustPreview: 8, rustResultSession: 15, total: 92 },
  build: 'typecheck + Vite passed; cargo build --locked --bin yuzora passed (reported by database agent)',
  securityFinding: 'schema valid, independent source verification VERIFIED; one local synthetic-password reproduction reported by database agent',
  executionPlanning: {
    date: plan.date,
    productImplementation: 'This documentation task did not implement product changes; all planned work packages await implementation acceptance.',
    counts: plan.counts,
    checks: 'scope coverage, implementation mappings, task references, acyclic requires/mockAfter DAG, unique effective writers, rendered tasks, filters, deep links, attachments',
    status: errors.length ? 'FAIL' : 'PASS',
    productTestsRerunForDocumentUpdate: false,
    concurrentWorkAlignment: plan.workspaceAlignment,
    historicalAuditResults: 'The 92 tests and build results above belong to the original audit, not this documentation update.'
  }
}
writeFileSync(resolve(dir, 'validation.json'), JSON.stringify(result, null, 2) + '\n')
console.log(JSON.stringify(result, null, 2))
dom.window.close()
if (errors.length) process.exit(1)
