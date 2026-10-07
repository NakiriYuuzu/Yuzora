/** Isolated dbStore sorting, no SQL/server/credentials or user storage.
 * Build: bun scripts/benchmark-database-sort.ts --build output/.../baseline.mjs
 * Run: YUZORA_PERF_OUTPUT=... bun scripts/benchmark-database-sort.ts baseline=...mjs candidate=...mjs
 */
import { strict as assert } from "node:assert"
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import type { DbQueryState, DbConnection, useDbStore } from "../src/state/dbStore"
import type { DbResultSessionOwner, DbResultPage, DbQueryRun, DbValue } from "../src/lib/types"

if (process.argv[2] === "--build") {
  assert(process.argv[3])
  const source = readFileSync("src/lib/ipc.ts", "utf8")
  const exports = [...source.matchAll(/^export\s+(?:async\s+)?(?:function|const)\s+(\w+)/gm)].map(match => match[1])
  assert(exports.length > 0, "Cannot discover the IPC exports")
  const result = await Bun.build({ entrypoints: ["src/state/dbStore.ts"], target: "bun",
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [{ name: "reject-native-ipc", setup(build) {
      build.onResolve({ filter: /^@\/lib\/ipc$/ }, () => ({ path: "no-native-ipc", namespace: "perf-fixture" }))
      build.onLoad({ filter: /.*/, namespace: "perf-fixture" }, () => ({ loader: "js", contents:
        exports.map(name => `export function ${name}() { throw new Error("Unexpected native IPC: ${name}") }`).join("\n") }))
    } }]
  })
  assert(result.success, result.logs.map(String).join("\n"))
  assert.equal(result.outputs.length, 1)
  await Bun.write(process.argv[3], result.outputs[0])
  process.exit(0)
}

Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: () => null, setItem: () => { throw new Error("Unexpected persistent write") }, removeItem: () => {}
} })
type Module = { useDbStore: typeof useDbStore; queryFor: (state: { queries: Record<string, DbQueryState> }, id: string | null) => DbQueryState; resultPageKey: (owner: DbResultSessionOwner) => string }
const implementations: { label: string; hash: string; module: Module }[] = []
for (const argument of process.argv.slice(2)) {
  const separator = argument.indexOf("=")
  assert(separator > 0, "Expected label=/path/module.mjs")
  const path = resolve(argument.slice(separator + 1))
  implementations.push({ label: argument.slice(0, separator),
    hash: createHash("sha256").update(readFileSync(path)).digest("hex"), module: await import(pathToFileURL(path).href) })
}
assert(implementations.length && process.env.YUZORA_PERF_OUTPUT)
const owner = { descriptorId: "benchmark-profile", connectionId: "benchmark-connection", connectionGeneration: "1",
  queryRunId: "benchmark-query", statementExecutionId: "benchmark-statement", resultSessionId: "benchmark-result" } as DbResultSessionOwner
const connection: DbConnection = { connId: owner.connectionId, descriptorId: owner.descriptorId, connectionGeneration: owner.connectionGeneration,
  kind: "sqlite", name: "fixture", title: "fixture", targetKey: "fixture" }
function query(module: Module, rows: DbValue[][], paged: boolean): DbQueryState {
  const result: DbQueryState["result"] = { kind: "select", columns: ["value", "index"], rows,
    truncated: false, affectedRows: null, effectOutcome: "none" }
  const base = { ...module.queryFor({ queries: {} }, null), sql: "SELECT fixture", result, lastSql: "SELECT fixture" }
  if (!paged) return base
  const page: DbResultPage = { owner, pageIndex: 0, columns: result.columns, rows, hasPrevious: false, hasNext: false,
    effectOutcome: "none", lifecycle: "complete", resultLimitReached: false }
  const run: DbQueryRun = { ...owner, statements: [{ statementExecutionId: owner.statementExecutionId, statementIndex: 0,
    sql: "SELECT fixture", effectOutcome: "none", result: { kind: "rows", affectedRows: null,
      resultSession: { owner, columns: result.columns, initialPage: page } } }], transactionMayBeOpen: false, connectionTerminated: false }
  return { ...base, runGroup: { owner, mode: "primary", units: [], status: "settled", run,
    activeStatementExecutionId: owner.statementExecutionId, startedAt: 0, cancelOutcome: null,
    resultPages: { [module.resultPageKey(owner)]: { page, loading: false, pageError: null, released: false, sort: null, sortBaseRows: rows } } } }
}
function rows(kind: string, count: number): DbValue[][] {
  const mixed: DbValue[] = [{ kind: "null" }, { kind: "integer", value: "9007199254740993" },
    { kind: "integer", value: "9007199254740992" }, { kind: "decimal", value: "-0.000" },
    { kind: "decimal", value: "1e3" }, { kind: "decimal", value: "not-a-number" },
    { kind: "text", value: "中文" }, { kind: "boolean", value: true }, { kind: "binary", hex: "ff" }]
  return Array.from({ length: count }, (_, i) => {
    const n = (i * 293) % count
    const value: DbValue = kind === "text" ? { kind: "text", value: `item-${n} 中文` }
      : kind === "mixed" ? { ...mixed[n % mixed.length] }
      : kind === "decimal" ? { kind: "decimal", value: `${i % 3 ? "" : "-"}${BigInt(n) * 998877665544332211n}.${String(n % 97).padStart(5, "0")}003400` }
      : { kind: "integer", value: String(kind === "repeated" ? n % 10 : n - Math.floor(count / 2)) }
    return [value, { kind: "integer", value: String(i) }]
  })
}
function stats(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b)
  const at = (p: number) => sorted[Math.ceil(sorted.length * p) - 1]
  return { p50: at(.5), p95: at(.95), p99: at(.99), min: sorted[0], max: sorted.at(-1) }
}
const results = []
for (const paged of [true, false]) for (const kind of ["small", "integer", "decimal", "repeated", "text", "mixed"]) {
  const input = rows(kind, kind === "small" ? 32 : 500)
  const original = JSON.stringify(input)
  const samples = []
  let expected: DbValue[][] | undefined
  const states = implementations.map(implementation => ({ ...implementation, query: query(implementation.module, input, paged) }))
  const reset = (implementation: (typeof states)[number]) => implementation.module.useDbStore.setState({
    connections: [connection], activeConnId: connection.connId, activeDescriptorId: owner.descriptorId,
    queryBuckets: { [owner.descriptorId]: implementation.query }, queries: { [connection.connId]: implementation.query }
  })
  const sorted = (implementation: (typeof states)[number]) => {
    const result = implementation.module.useDbStore.getState().queryBuckets[owner.descriptorId].result
    assert(result?.kind === "select")
    return result.rows
  }
  for (const implementation of states) {
    reset(implementation)
    await implementation.module.useDbStore.getState().sortResult(0, paged ? owner : undefined)
    if (expected) assert.deepEqual(sorted(implementation), expected)
    else expected = sorted(implementation)
    await implementation.module.useDbStore.getState().sortResult(0, paged ? owner : undefined)
    await implementation.module.useDbStore.getState().sortResult(0, paged ? owner : undefined)
    assert.equal(sorted(implementation), input, "Clearing a sort must restore the original row array")
    for (let warmup = 0; warmup < 50; warmup++) {
      reset(implementation)
      await implementation.module.useDbStore.getState().sortResult(0, paged ? owner : undefined)
    }
  }
  for (let repetition = 0; repetition < 7; repetition++) for (const implementation of repetition % 2 ? states.toReversed() : states) {
    const durations = []
    const memoryBefore = process.memoryUsage()
    const cpuBefore = process.cpuUsage()
    const iterations = kind === "small" ? 5000 : kind === "text" ? 1000 : 300
    for (let i = 0; i < iterations; i++) {
      reset(implementation)
      const start = performance.now()
      await implementation.module.useDbStore.getState().sortResult(0, paged ? owner : undefined)
      durations.push(performance.now() - start)
    }
    const cpu = process.cpuUsage(cpuBefore)
    samples.push({ label: implementation.label, repetition, iterations, operationMs: stats(durations),
      cpuMsPerCall: (cpu.user + cpu.system) / 1000 / iterations, memoryBefore, memoryAfter: process.memoryUsage() })
  }
  assert.equal(JSON.stringify(input), original, "Mutated original rows")
  results.push({ scenario: `${paged ? "paged" : "legacy"}-${kind}`, rowCount: input.length, samples,
    summaries: implementations.map(implementation => {
      const selected = samples.filter(sample => sample.label === implementation.label)
      return { label: implementation.label, medianMs: stats(selected.map(sample => sample.operationMs.p50)),
        p95Ms: stats(selected.map(sample => sample.operationMs.p95)), cpuMsPerCall: stats(selected.map(sample => sample.cpuMsPerCall)) }
    }) })
}
writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify({ recordedAt: new Date().toISOString(), runtime: process.versions,
  scope: "Actual dbStore sortResult with production defines and native IPC rejected. Rows limited to backend's 500-row page. CPU includes identical state reset; no server/DOM latency or leak-freedom claim.",
  repetitions: 7, implementations: implementations.map(({ label, hash }) => ({ label, hash })), results }, null, 2) + "\n", { flag: "wx" })
console.log(JSON.stringify(results.map(({ scenario, summaries }) => ({ scenario, summaries })), null, 2))
