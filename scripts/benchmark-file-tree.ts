/** Actual store benchmark with isolated in-memory directory reads; no browser or native IPC.
 * Build: bun scripts/benchmark-file-tree.ts --build output/.../baseline.mjs
 * Run: YUZORA_PERF_OUTPUT=... bun scripts/benchmark-file-tree.ts baseline=...mjs candidate=...mjs
 */
import { strict as assert } from "node:assert"
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import type { FileNode } from "../src/lib/types"
import type { useFileTreeStore as Store, WorkspaceTree } from "../src/state/fileTreeStore"

if (process.argv[2] === "--build") {
  assert(process.argv[3], "An output filename is required")
  const result = await Bun.build({
    entrypoints: ["src/state/fileTreeStore.ts"], target: "bun",
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [{ name: "fixture-listing", setup(build) {
      build.onResolve({ filter: /\/lib\/ipc$/ }, () => ({ path: "directory-fixture", namespace: "perf-fixture" }))
      build.onLoad({ filter: /.*/, namespace: "perf-fixture" }, () => ({ loader: "js", contents:
        "export const listDir = async (path) => globalThis.__yuzoraFileTreeBenchmarkList(path);" }))
    } }]
  })
  assert(result.success, result.logs.map(String).join("\n"))
  assert.equal(result.outputs.length, 1)
  await Bun.write(process.argv[3], result.outputs[0])
  process.exit(0)
}

const globalFixture = globalThis as typeof globalThis & { __yuzoraFileTreeBenchmarkList?: (path: string) => FileNode[] }
const implementations: { label: string; hash: string; store: typeof Store }[] = []
for (const argument of process.argv.slice(2)) {
  const split = argument.indexOf("=")
  assert(split > 0, "Expected label=/path/module.mjs")
  const path = resolve(argument.slice(split + 1))
  const module = await import(pathToFileURL(path).href)
  implementations.push({ label: argument.slice(0, split), hash: createHash("sha256").update(readFileSync(path)).digest("hex"), store: module.useFileTreeStore })
}
assert(implementations.length && process.env.YUZORA_PERF_OUTPUT)
function entry(path: string, isDir: boolean): FileNode {
  return { path, name: path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1), isDir }
}
function fixture(root: string, groups: number, removed: number, unchanged = false) {
  const separator = root.includes("\\") ? "\\" : "/"
  const directories = Array.from({ length: groups }, (_, i) => `${root}${separator}group-${i}`)
  const childrenByDir: Record<string, FileNode[]> = {}
  for (const directory of directories) {
    const nested = Array.from({ length: 8 }, (_, i) => `${directory}${separator}nested-${i}`)
    childrenByDir[directory] = nested.map(path => entry(path, true))
    for (const path of nested) childrenByDir[path] = [entry(`${path}${separator}file.ts`, false)]
  }
  // Collapsed, previously visited tree: ensureTree revalidates only the root.
  const tree: WorkspaceTree = { rootNodes: directories.map(path => entry(path, true)), childrenByDir, expandedDirs: new Set(), scrollTop: 37 }
  const listing = unchanged ? [...tree.rootNodes] : [...tree.rootNodes.slice(removed), entry(`${root}${separator}new-file.ts`, false)]
  return { root, tree, listing, removed, directories }
}
function stats(values: number[]) {
  const ordered = values.toSorted((a, b) => a - b)
  const at = (p: number) => ordered[Math.ceil(ordered.length * p) - 1]
  return { p50: at(.5), p95: at(.95), p99: at(.99), min: ordered[0], max: ordered.at(-1) }
}
const scenarios = [
  { name: "small-root-change", data: fixture("/fixture", 4, 0), iterations: 50000 },
  { name: "large-root-change", data: fixture("/fixture", 240, 0), iterations: 100 },
  { name: "unchanged-large", data: fixture("/fixture", 240, 0, true), iterations: 50000 },
  { name: "remove-one-subtree", data: fixture("/fixture", 240, 1), iterations: 100 },
  { name: "remove-two-subtrees", data: fixture("/fixture", 240, 2), iterations: 100 },
  { name: "remove-four-subtrees", data: fixture("/fixture", 240, 4), iterations: 100 },
  { name: "remove-120-subtrees", data: fixture("/fixture", 240, 120), iterations: 10 },
  { name: "windows-remove-120", data: fixture("C:\\fixture", 240, 120), iterations: 10 }
]
const results = []
try {
  for (const scenario of scenarios) {
    const { root, tree, listing, directories, removed } = scenario.data
    let reads = 0
    globalFixture.__yuzoraFileTreeBenchmarkList = path => { assert.equal(path, root); reads++; return listing }
    const run = async (store: typeof Store) => {
      store.setState({ trees: { [root]: tree }, preciseRevision: null })
      const start = performance.now()
      await store.getState().ensureTree(root)
      return performance.now() - start
    }
    let expected: WorkspaceTree | undefined
    for (const implementation of implementations) {
      for (let warmup = 0; warmup < 20; warmup++) await run(implementation.store)
      const result = implementation.store.getState().trees[root]
      assert.deepEqual(result.rootNodes, listing)
      assert.equal(Object.keys(result.childrenByDir).length, (directories.length - removed) * 9)
      assert.equal(result.scrollTop, 37)
      assert.equal(result.expandedDirs.size, 0)
      if (expected) assert.deepEqual(result, expected)
      else expected = result
    }
    const samples = []
    for (let repetition = 0; repetition < 7; repetition++) {
      for (const implementation of repetition % 2 ? implementations.toReversed() : implementations) {
        const durations: number[] = []
        const readsBefore = reads
        const memoryBefore = process.memoryUsage()
        const cpuBefore = process.cpuUsage()
        let notifications = 0
        const unsubscribe = implementation.store.subscribe(() => { notifications++ })
        try {
          for (let iteration = 0; iteration < scenario.iterations; iteration++) durations.push(await run(implementation.store))
        } finally { unsubscribe() }
        const cpu = process.cpuUsage(cpuBefore)
        assert.equal(reads - readsBefore, scenario.iterations)
        samples.push({ label: implementation.label, repetition, iterations: scenario.iterations,
          operationMs: stats(durations), cpuMsPerCall: (cpu.user + cpu.system) / 1000 / scenario.iterations,
          reads: reads - readsBefore, notifications, memoryBefore, memoryAfter: process.memoryUsage() })
      }
    }
    assert.equal(Object.keys(tree.childrenByDir).length, directories.length * 9, "Mutated old cached tree")
    results.push({ scenario: scenario.name, cachedDirectories: directories.length * 9, samples,
      summaries: implementations.map(implementation => {
        const selected = samples.filter(sample => sample.label === implementation.label)
        return { label: implementation.label, medianOperationMs: stats(selected.map(sample => sample.operationMs.p50)),
          p95OperationMs: stats(selected.map(sample => sample.operationMs.p95)), cpuMsPerCall: stats(selected.map(sample => sample.cpuMsPerCall)),
          notifications: selected.map(sample => sample.notifications) }
      }) })
  }
} finally { delete globalFixture.__yuzoraFileTreeBenchmarkList }
writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify({ recordedAt: new Date().toISOString(), runtime: process.versions,
  scope: "Production-defined store with only listDir mocked; no native filesystem or DOM. CPU includes identical state reset; operation timing excludes that reset. Memory samples do not prove leak freedom.",
  implementations: implementations.map(({ label, hash }) => ({ label, hash })), repetitions: 7, results }, null, 2) + "\n", { flag: "wx" })
console.log(JSON.stringify(results.map(({ scenario, summaries }) => ({ scenario, summaries })), null, 2))
