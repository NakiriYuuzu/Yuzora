/** Actual terminal transport with in-memory IPC; no native server or GUI.
 * Build: bun scripts/benchmark-terminal-input.ts --build output/.../baseline.mjs
 * Run: YUZORA_PERF_OUTPUT=... bun scripts/benchmark-terminal-input.ts baseline=...mjs
 */
import { strict as assert } from "node:assert"
import { createHash } from "node:crypto"
import { readFileSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import type { TerminalTransport } from "../src/terminal/terminalTransport"

if (process.argv[2] === "--build") {
  const fixtures: Record<string, string> = {
    "@/lib/herdrIpc": `
      export const herdrTerminalOpen = async (args) => globalThis.__yuzoraInputFixture.open(args);
      export const herdrTerminalInput = async (id, text) => globalThis.__yuzoraInputFixture.input(id, text);
      export const herdrTerminalRelease = async (id) => globalThis.__yuzoraInputFixture.release(id);
      export const herdrTerminalResize = async () => {};
      export const herdrTerminalScroll = async () => {};
    `,
    "./herdrScrollIpc": "export const readPaneScroll = async () => { throw Error('unexpected scroll') }; export const setPaneScroll = readPaneScroll;",
    "./herdrTerminalDiagnostics": "export const recordHerdrTerminalMetric = () => {}; export const timeHerdrTerminalIpc = () => { throw Error('unexpected scroll metric') };"
  }
  const built = await Bun.build({
    entrypoints: ["src/terminal/terminalTransport.ts"], target: "bun",
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [{ name: "terminal-input-fixtures", setup(build) {
      build.onResolve({ filter: /herdrIpc$|herdrScrollIpc$|herdrTerminalDiagnostics$/ }, (args) => {
        assert(args.path in fixtures, `Unexpected fixture import ${args.path}`)
        return { path: args.path, namespace: "input-fixture" }
      })
      build.onLoad({ filter: /.*/, namespace: "input-fixture" }, (args) => ({ loader: "js", contents: fixtures[args.path] }))
    } }]
  })
  assert(built.success, built.logs.map(String).join("\n"))
  assert.equal(built.outputs.length, 1)
  assert(process.argv[3])
  writeFileSync(process.argv[3], await built.outputs[0].text(), { flag: "wx" })
  process.exit(0)
}

const argument = process.argv[2]
assert(argument?.includes("=") && process.env.YUZORA_PERF_OUTPUT)
const separator = argument.indexOf("=")
const label = argument.slice(0, separator)
const modulePath = resolve(argument.slice(separator + 1))
const module = await import(pathToFileURL(modulePath).href)
let sequence = 0
let frames = 0
let codeUnits = 0
const active = new Set<string>()
let hold: Promise<void> | null = null
const fixture = {
  open: (args: { target: string; cols: number; rows: number }) => {
    const sessionId = `owned-${++sequence}`
    active.add(sessionId)
    return { sessionId, target: args.target, cols: args.cols, rows: args.rows, mode: "control", role: "controller", takeover: true }
  },
  input: (id: string, text: string) => {
    assert(active.has(id))
    frames += 1
    codeUnits += text.length
    return hold
  },
  release: (id: string) => { assert(active.delete(id)) }
}
Object.assign(globalThis, { __yuzoraInputFixture: fixture })
if (process.argv.includes("--lifecycle")) {
  const originalEncode = TextEncoder.prototype.encode
  let encodes = 0
  TextEncoder.prototype.encode = function (input?: string) {
    encodes += 1
    return originalEncode.call(this, input)
  }
  const cycles = []
  try {
    for (let cycle = 0; cycle < 200; cycle++) {
      const encodesBefore = encodes
      const framesBefore = frames
      const transport: TerminalTransport = module.createHerdrTerminalTransport({ terminalId: "owned-lifecycle" })
      await transport.open({ cols: 80, rows: 24, onEvent: event => { assert.notEqual(event.type, "error") } })
      await Promise.all(Array.from({ length: 128 }, () => transport.write("x")))
      await Promise.all([transport.write("中"), transport.write("😀"), transport.paste("x")])
      let unblock!: () => void
      hold = new Promise<void>(resolve => { unblock = resolve })
      const head = transport.write("head")
      await Promise.resolve()
      const pending = Array.from({ length: 128 }, () => transport.write("z"))
      await transport.dispose?.()
      hold = null
      unblock()
      await Promise.all([head, ...pending])
      assert.equal(frames - framesBefore, 4)
      assert.equal(active.size, 0)
      assert.equal(transport.isDisposed?.(), true)
      const afterClose = frames
      await transport.write("never sent")
      assert.equal(frames, afterClose)
      cycles.push({ cycle, warmup: cycle < 100, encodes: encodes - encodesBefore,
        frames: frames - framesBefore, activeAfterClose: active.size, memory: process.memoryUsage() })
    }
  } finally {
    TextEncoder.prototype.encode = originalEncode
  }
  writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify({ label, cycles,
    scope: "100 warmup +100 transport open/write/dispose cycles in one process; controlled IPC. Encode invocation and active-fixture counts are exact. Natural-GC memory samples are diagnostic, not native App leak evidence."
  }, null, 2) + "\n", { flag: "wx" })
  console.log(`${label}: 200 input lifecycle cycles passed`)
  process.exit(0)
}
const scenarios = [
  { name: "ascii-key", text: "x", kind: "write", iterations: 30000 },
  { name: "cjk-key", text: "中", kind: "write", iterations: 30000 },
  { name: "emoji-key", text: "😀", kind: "write", iterations: 30000 },
  { name: "ascii-command", text: "echo terminal benchmark\r", kind: "write", iterations: 20000 },
  { name: "ascii-paste-4k", text: "0123456789abcdef".repeat(256), kind: "paste", iterations: 2000 },
  { name: "ascii-paste-64k", text: "0123456789abcdef".repeat(4096), kind: "paste", iterations: 512 },
  { name: "cjk-paste", text: "中文測試".repeat(4096), kind: "paste", iterations: 512 },
  { name: "emoji-paste", text: "😀🚀".repeat(4096), kind: "paste", iterations: 512 },
  { name: "mixed-paste", text: "abc 中文 😀\n".repeat(512), kind: "paste", iterations: 1000 },
  { name: "ascii-prefix-unicode", text: "x".repeat(4096) + "中文😀", kind: "paste", iterations: 2000 },
  { name: "lone-surrogates", text: "a\ud800b\udc00中".repeat(512), kind: "paste", iterations: 1000 },
  { name: "queued-typing", text: "echo 中文 😀\r", kind: "queued", iterations: 500 }
] as const
const results = []
for (const scenario of scenarios) {
  const iterations = scenario.iterations * 8
  const transport: TerminalTransport = module.createHerdrTerminalTransport({ terminalId: "owned-input" })
  await transport.open({ cols: 80, rows: 24, onEvent: (event) => { assert.notEqual(event.type, "error") } })
  const samples = []
  for (let pass = 0; pass < 8; pass++) {
    const framesBefore = frames
    const unitsBefore = codeUnits
    const memoryBefore = process.memoryUsage()
    const cpuBefore = process.cpuUsage()
    const started = performance.now()
    for (let iteration = 0; iteration < iterations; iteration++) {
      if (scenario.kind === "queued") {
        let unblock!: () => void
        hold = new Promise<void>(resolve => { unblock = resolve })
        const head = transport.write("head")
        await Promise.resolve()
        const tail = [...scenario.text].map(character => transport.write(character))
        hold = null
        unblock()
        await Promise.all([head, ...tail])
      } else {
        await transport[scenario.kind](scenario.text)
      }
    }
    const wallMs = performance.now() - started
    const cpu = process.cpuUsage(cpuBefore)
    const expectedFrames = iterations * (scenario.kind === "queued" ? 2 : 1)
    const expectedUnits = iterations * (scenario.text.length + (scenario.kind === "paste" ? 12 : scenario.kind === "queued" ? 4 : 0))
    assert.equal(frames - framesBefore, expectedFrames)
    assert.equal(codeUnits - unitsBefore, expectedUnits)
    samples.push({ pass, warmup: pass === 0, cpuMs: (cpu.user + cpu.system) / 1000, wallMs,
      frames: expectedFrames, codeUnits: expectedUnits, memoryBefore, memoryAfter: process.memoryUsage() })
  }
  await transport.dispose?.()
  assert.equal(active.size, 0)
  results.push({ scenario: scenario.name, iterations, samples })
}
writeFileSync(process.env.YUZORA_PERF_OUTPUT!, JSON.stringify({ label, runtime: process.versions,
  sha256: createHash("sha256").update(readFileSync(modulePath)).digest("hex"),
  scope: "Bun transport enqueue/drain/paste CPU with controlled in-memory IPC. Batch durations are not GUI input latency. Natural-GC memory samples are not leak evidence.",
  activeFixturesAfterClose: active.size, results
}, null, 2) + "\n", { flag: "wx" })
console.log(`${label}: ${results.length} transport scenarios passed with no active fixture after close`)
