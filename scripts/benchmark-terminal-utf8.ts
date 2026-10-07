/** Pure byte-count CPU benchmark, not terminal transport or WebView rendering.
 * YUZORA_PERF_OUTPUT=... bun scripts/benchmark-terminal-utf8.ts baseline=...mjs candidate=...mjs
 * Build both modules identically with `bun build src/terminal/terminalOutputQueue.ts --target=bun`.
 */
import { strict as assert } from "node:assert"
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"

const implementations: { label: string; hash: string; count: (value: string) => number }[] = []
for (const argument of process.argv.slice(2)) {
  const split = argument.indexOf("=")
  assert(split > 0, "Expected label=/path/module.mjs")
  const path = resolve(argument.slice(split + 1))
  const module = await import(pathToFileURL(path).href)
  assert.equal(typeof module.utf8Length, "function")
  implementations.push({ label: argument.slice(0, split),
    hash: createHash("sha256").update(readFileSync(path)).digest("hex"), count: module.utf8Length })
}
assert(implementations.length > 0 && process.env.YUZORA_PERF_OUTPUT)
function stats(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b)
  const percentile = (p: number) => sorted[Math.ceil(sorted.length * p) - 1]
  return { p50: percentile(.5), p95: percentile(.95), p99: percentile(.99), min: sorted[0], max: sorted.at(-1) }
}
const fixtures = [
  { name: "tiny-ascii", text: "ok" },
  { name: "short-ascii", text: "Working on terminal output now.\r\n" },
  { name: "ascii-31", text: "x".repeat(31) },
  { name: "ascii-32", text: "x".repeat(32) },
  { name: "ansi-256", text: "\x1b[32mCompiler finished.\x1b[0m\r\n".repeat(10) },
  { name: "ascii-4k", text: "terminal output\r\n".repeat(256) },
  { name: "ascii-64k", text: "0123456789abcdef".repeat(4096) },
  { name: "cjk-4k", text: "終端機輸出效能量測\r\n".repeat(256) },
  { name: "emoji-4k", text: "😀🚀🧪 output\r\n".repeat(256) },
  { name: "mixed-4k", text: "\x1b[32mBuild 程式檔案 😀\x1b[0m\r\n".repeat(128) },
  { name: "ascii-prefix-cjk", text: "x".repeat(4096) + "中文😀" },
  { name: "lone-surrogates", text: "a\ud800b\udc00中\ud83d\ude00".repeat(256) }
]
const results = []
let checksum = 0
for (const fixture of fixtures) {
  const expected = new TextEncoder().encode(fixture.text).length
  const iterations = Math.max(256, Math.min(1_000_000, Math.ceil(8_000_000 / fixture.text.length)))
  for (const implementation of implementations) {
    assert.equal(implementation.count(fixture.text), expected)
    for (let warmup = 0; warmup < 1000; warmup++) checksum += implementation.count(fixture.text)
  }
  const samples = []
  for (let repetition = 0; repetition < 7; repetition++) {
    for (const implementation of repetition % 2 ? implementations.toReversed() : implementations) {
      const memoryBefore = process.memoryUsage()
      const cpuBefore = process.cpuUsage()
      const start = performance.now()
      for (let iteration = 0; iteration < iterations; iteration++) checksum += implementation.count(fixture.text)
      const elapsedMs = performance.now() - start
      const cpu = process.cpuUsage(cpuBefore)
      samples.push({ label: implementation.label, repetition, iterations, elapsedMs,
        cpuMs: (cpu.user + cpu.system) / 1000, memoryBefore, memoryAfter: process.memoryUsage() })
    }
  }
  results.push({ scenario: fixture.name, codeUnits: fixture.text.length, bytes: expected, samples,
    summaries: implementations.map(implementation => {
      const selected = samples.filter(sample => sample.label === implementation.label)
      return { label: implementation.label,
        meanMsPerCall: stats(selected.map(sample => sample.elapsedMs / sample.iterations)),
        cpuMsPerCall: stats(selected.map(sample => sample.cpuMs / sample.iterations)) }
    }) })
}
writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify({
  recordedAt: new Date().toISOString(), runtime: process.versions, platform: process.platform, arch: process.arch,
  scope: "Pure UTF-8 byte counting. Percentiles describe seven batch means, not App p95. Memory is natural-GC sampling, not a leak verdict.",
  repetitions: 7, implementations: implementations.map(({ label, hash }) => ({ label, hash })), checksum, results
}, null, 2) + "\n", { flag: "wx" })
console.log(JSON.stringify(results.map(({ scenario, summaries }) => ({ scenario, summaries })), null, 2))
