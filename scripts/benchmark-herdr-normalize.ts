/**
 * Isolated CPU microbenchmark; does not connect to HERDR or measure WebView paint.
 * Build each implementation with the same `bun build --target=bun` command, then:
 * YUZORA_PERF_OUTPUT=/path/result.json bun scripts/benchmark-herdr-normalize.ts label=/path/module.mjs [...]
 */
import { strict as assert } from "node:assert"
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import type { HerdrSnapshot, HerdrSnapshotResult } from "../src/lib/herdrTypes"

type Normalize = (input: HerdrSnapshotResult, session: string) => HerdrSnapshot
const implementations: { label: string; hash: string; normalize: Normalize }[] = []
for (const argument of process.argv.slice(2)) {
  const separator = argument.indexOf("=")
  assert(separator > 0, "Expected label=/path/module.mjs")
  const label = argument.slice(0, separator)
  const path = resolve(argument.slice(separator + 1))
  const module = await import(pathToFileURL(path).href)
  assert.equal(typeof module.normalizeHerdrSnapshot, "function")
  implementations.push({ label, hash: createHash("sha256").update(readFileSync(path)).digest("hex"), normalize: module.normalizeHerdrSnapshot })
}
assert(implementations.length > 0, "At least one built implementation is required")
assert(process.env.YUZORA_PERF_OUTPUT, "YUZORA_PERF_OUTPUT is required")

function fixture(panes: number, panesPerTab = 2, partial = false): HerdrSnapshotResult {
  const tabCount = Math.ceil(panes / panesPerTab)
  const workspaceCount = Math.ceil(tabCount / 4)
  const workspaces = Array.from({ length: workspaceCount }, (_, index) => ({
    workspace_id: `ws-${index}`, label: `Workspace ${index}`, number: index,
    focused: index === 0, active_tab_id: `tab-${index * 4}`,
    worktree: { checkout_path: `/fixture/project-${index}`, repo_root: `/fixture/project-${index}`, repo_key: `repo-${index}`, repo_name: `project-${index}`, is_linked_worktree: false }
  }))
  const tabs = Array.from({ length: tabCount }, (_, index) => ({
    tab_id: `tab-${index}`, workspace_id: `ws-${Math.floor(index / 4)}`,
    label: `Agent ${index}`, number: index % 4, focused: index === 0,
    agent_status: "working"
    // Missing pane_count exercises the supported count derivation path.
  }))
  const paneRecords = Array.from({ length: panes }, (_, index) => ({
    pane_id: `pane-${index}`, terminal_id: `terminal-${index}`,
    tab_id: `tab-${Math.floor(index / panesPerTab)}`,
    workspace_id: `ws-${Math.floor(index / panesPerTab / 4)}`,
    title: `Agent terminal ${index}`, cwd: `/fixture/project-${Math.floor(index / panesPerTab / 4)}`,
    agent_status: index % 3 === 0 ? "blocked" : "working", focused: index === 1
  }))
  return { protocol: 22, version: "0.9.3", snapshot: {
    focused_workspace_id: "ws-0", focused_tab_id: "tab-0", focused_pane_id: "pane-1",
    workspaces, tabs: partial ? [] : tabs, panes: paneRecords,
    agents: paneRecords.map(pane => ({ ...pane, display_agent: "codex" }))
  } }
}

function percentiles(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b)
  const at = (p: number) => sorted[Math.max(0, Math.ceil(sorted.length * p) - 1)]
  return { p50: at(.5), p95: at(.95), p99: at(.99), min: sorted[0], max: sorted.at(-1)! }
}

const scenarios = [
  { name: "small-4", input: fixture(4), iterations: 50000 },
  { name: "daily-24", input: fixture(24), iterations: 30000 },
  { name: "busy-120", input: fixture(120), iterations: 3000 },
  { name: "stress-480", input: fixture(480), iterations: 500 },
  { name: "single-tab-128", input: fixture(128, 128), iterations: 10000 },
  { name: "partial-120", input: fixture(120, 2, true), iterations: 3000 }
]
const results = []
let checksum = 0
for (const scenario of scenarios) {
  const canonical = JSON.stringify(scenario.input)
  const expected = implementations[0].normalize(scenario.input, "benchmark")
  for (const implementation of implementations) {
    assert.deepEqual(implementation.normalize(scenario.input, "benchmark"), expected)
    for (let warmup = 0; warmup < 1000; warmup++) implementation.normalize(scenario.input, "benchmark")
  }
  const samples = []
  for (let repetition = 0; repetition < 7; repetition++) {
    // Alternate order to avoid systematically favoring the later implementation.
    const order = repetition % 2 ? implementations.toReversed() : implementations
    for (const implementation of order) {
      const durations: number[] = []
      const memoryBefore = process.memoryUsage()
      const cpuBefore = process.cpuUsage()
      const start = performance.now()
      for (let iteration = 0; iteration < scenario.iterations; iteration++) {
        const before = performance.now()
        const snapshot = implementation.normalize(scenario.input, "benchmark")
        durations.push(performance.now() - before)
        checksum += snapshot.tabs.length + snapshot.agents.length
      }
      const elapsedMs = performance.now() - start
      const cpu = process.cpuUsage(cpuBefore)
      samples.push({ label: implementation.label, repetition, iterations: scenario.iterations,
        elapsedMs, cpuMs: (cpu.user + cpu.system) / 1000,
        perOperationMs: percentiles(durations), memoryBefore, memoryAfter: process.memoryUsage() })
    }
  }
  assert.equal(JSON.stringify(scenario.input), canonical, "Normalization mutated its input")
  results.push({ scenario: scenario.name, inputBytes: Buffer.byteLength(canonical),
    inputHash: createHash("sha256").update(canonical).digest("hex"), samples,
    summaries: implementations.map(implementation => {
      const selected = samples.filter(sample => sample.label === implementation.label)
      return { label: implementation.label,
        batchMeanMs: percentiles(selected.map(sample => sample.elapsedMs / sample.iterations)),
        cpuMsPerOperation: percentiles(selected.map(sample => sample.cpuMs / sample.iterations)),
        perOperationP95Ms: percentiles(selected.map(sample => sample.perOperationMs.p95)) }
    }) })
}
const output = { recordedAt: new Date().toISOString(), runtime: process.versions,
  platform: process.platform, arch: process.arch, implementations: implementations.map(({ label, hash }) => ({ label, hash })),
  scope: "Pure HERDR snapshot normalization; no IPC, rendering, or native runtime. RSS/heap samples include natural GC; they do not prove leak freedom.",
  repetitions: 7, warmupPerScenarioAndImplementation: 1000, checksum, results }
writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify(output, null, 2) + "\n", { flag: "wx" })
console.log(JSON.stringify(results.map(result => ({ scenario: result.scenario, summaries: result.summaries })), null, 2))
