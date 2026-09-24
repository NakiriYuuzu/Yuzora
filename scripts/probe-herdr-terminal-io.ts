/**
 * Isolated Herdr terminal I/O probe: IPC latency and modified-key delivery.
 *
 * Starts its own Herdr server under temporary XDG/APPDATA roots and a unique
 * session name. It never connects to, stops or mutates a user's server.
 *
 * usage: bun scripts/probe-herdr-terminal-io.ts <herdr-binary> [--out file.json] [--skip-input] [--iterations N]
 */
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdtemp, mkdir, writeFile, realpath } from "node:fs/promises"
import { join, relative, isAbsolute } from "node:path"
import { tmpdir, platform, arch, release } from "node:os"
import { createConnection, type Socket } from "node:net"
import { createInterface } from "node:readline"
import { removeRuntimeFixture } from "./runtime-fixture-cleanup"

type Row = Record<string, unknown>
type Stats = { n: number; p50: number; p95: number; max: number }
type Verdict = "ok" | "split" | "raw-w32" | "none" | "other"

const args = process.argv.slice(2)
const flag = (name: string) => args.includes(name)
const option = (name: string) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined }
if (!args[0] || args[0].startsWith("--")) {
  console.error("usage: bun scripts/probe-herdr-terminal-io.ts <herdr-binary> [--out file.json] [--skip-input] [--iterations N]")
  process.exit(2)
}
const binary = await realpath(args[0])
const iterations = Math.max(5, Number(option("--iterations") ?? 50))
const windows = process.platform === "win32"
const session = `yz-probe-${Date.now().toString(36)}`
const root = await realpath(await mkdtemp(join(windows ? tmpdir() : "/tmp", "yz-probe-")))
const shell = windows ? (hasCommand("pwsh.exe") ? "pwsh.exe" : "powershell.exe") : "/bin/sh"
const env: NodeJS.ProcessEnv = Object.fromEntries(Object.entries(process.env)
  .filter(([key]) => !key.startsWith("HERDR_") && !["ENV", "BASH_ENV"].includes(key)))
Object.assign(env, {
  XDG_CONFIG_HOME: join(root, "cfg"), XDG_STATE_HOME: join(root, "state"), XDG_RUNTIME_DIR: join(root, "run"),
  HERDR_CONFIG_PATH: join(root, "config.toml"), HISTFILE: join(root, "shell-history"), SHELL: shell,
})
if (windows) Object.assign(env, { APPDATA: join(root, "cfg"), LOCALAPPDATA: join(root, "state") })

const children: ChildProcessWithoutNullStreams[] = []
const sockets: Socket[] = []
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))
function check(ok: unknown, message: string): asserts ok { if (!ok) throw new Error(message) }
function hasCommand(name: string) {
  const probe = spawnSync(windows ? "where" : "which", [name], { stdio: "ignore" })
  return probe.status === 0
}
function stats(samples: number[]): Stats {
  const sorted = [...samples].sort((a, b) => a - b)
  const at = (q: number) => sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] : 0
  const round = (v: number) => Math.round(v * 100) / 100
  return { n: sorted.length, p50: round(at(0.5)), p95: round(at(0.95)), max: round(sorted.at(-1) ?? 0) }
}
function start(argv: string[]) {
  const child = spawn(binary, ["--session", session, ...argv], { cwd: join(root, "work"), env, stdio: "pipe" })
  children.push(child)
  return child
}
async function command(argv: string[], withSession = true) {
  const child = withSession ? start(argv) : spawn(binary, argv, { cwd: join(root, "work"), env, stdio: "pipe" })
  if (!withSession) children.push(child)
  let output = "", error = ""
  child.stdout.on("data", chunk => { output += chunk })
  child.stderr.on("data", chunk => { error += chunk })
  const timer = setTimeout(() => child.kill(), 15_000)
  try {
    await new Promise<void>((resolve, reject) => {
      child.once("error", reject)
      child.once("close", code => code === 0 ? resolve() : reject(new Error(`${argv.join(" ")}: ${error.slice(-500)}`)))
    })
    return JSON.parse(output)
  } finally { clearTimeout(timer) }
}
/** Bounded line stream: keeps only unmatched rows from the last few seconds. */
function lines(input: NodeJS.ReadableStream) {
  let rows: Row[] = []
  let failure: Error | null = null
  const reader = createInterface({ input })
  reader.on("line", line => {
    try { rows.push(JSON.parse(line)); if (rows.length > 400) rows = rows.slice(-200) }
    catch (error) { failure = error as Error }
  })
  input.on("error", error => { failure = error })
  return {
    clear() { rows = [] },
    async wait(accept: (row: Row) => boolean, timeoutMs = 10_000) {
      const deadline = Date.now() + timeoutMs
      while (Date.now() < deadline) {
        if (failure) throw failure
        const index = rows.findIndex(accept)
        if (index >= 0) { const [row] = rows.splice(index, 1); return row }
        await sleep(2)
      }
      throw new Error("probe response timeout")
    },
  }
}

const result: Record<string, unknown> = {
  host: { os: process.platform, arch: arch(), release: release(), shell, binary },
  latency: {} as Record<string, unknown>,
  input: [] as unknown[],
}
let server: ChildProcessWithoutNullStreams | undefined
try {
  for (const name of ["cfg", "state", "run", "work"]) await mkdir(join(root, name))
  await writeFile(join(root, "config.toml"), `onboarding = false
[remote]
manage_ssh_config = false
[terminal]
default_shell = "${shell.replaceAll("\\", "\\\\")}"
shell_mode = "non_login"
[session]
resume_agents_on_restore = false
[update]
version_check = false
manifest_check = false
`)
  server = start(["server"])
  server.stdout.resume(); server.stderr.resume()
  let status: { server?: { running?: boolean; compatible?: boolean; version?: string; protocol?: number; socket?: string } } | undefined
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    status = await command(["status", "--json"])
    if (status?.server?.running) break
    await sleep(100)
  }
  check(status?.server?.running && status.server.compatible === true, "isolated server did not start")
  check(typeof status.server.socket === "string", "missing isolated socket marker")
  const socketRelative = relative(root, status.server.socket)
  check(socketRelative && !isAbsolute(socketRelative) && !socketRelative.startsWith(".."), "refuse a socket outside the isolated root")
  Object.assign(result.host as Row, { herdr: status.server.version, protocol: status.server.protocol })
  const socketPath = windows ? "\\\\.\\pipe\\" + status.server.socket : status.server.socket

  let id = 0
  async function api<T = Row>(method: string, params: Row = {}): Promise<T> {
    const socket = createConnection(socketPath)
    sockets.push(socket)
    const stream = lines(socket)
    await new Promise<void>((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject) })
    const requestId = `probe-${++id}`
    socket.write(JSON.stringify({ id: requestId, method, params }) + "\n")
    const response = await stream.wait(row => row.id === requestId)
    socket.destroy()
    check(!response.error, `${method}: ${JSON.stringify(response.error)}`)
    return response.result as T
  }
  async function timed(samples: number[], run: () => Promise<unknown>) {
    const at = performance.now(); await run(); samples.push(performance.now() - at)
  }
  const latency = result.latency as Record<string, unknown>

  // Part A — the fixed costs Yuzora pays per call.
  const spawnSamples: number[] = []
  for (let i = 0; i < 20; i++) await timed(spawnSamples, () => command(["session", "list", "--json"], false))
  latency.sessionListSpawnMs = stats(spawnSamples)
  const pingSamples: number[] = []
  for (let i = 0; i < iterations; i++) await timed(pingSamples, () => api("ping"))
  latency.pingRoundTripMs = stats(pingSamples)

  const created = await api<{ root_pane: { terminal_id: string; pane_id?: string } }>("workspace.create",
    { cwd: join(root, "work"), label: "Yuzora IO probe", focus: true })
  const terminalId = created.root_pane.terminal_id
  const snapshot = (await api<{ snapshot: { panes: Array<{ pane_id: string; terminal_id: string }> } }>("session.snapshot")).snapshot
  const paneId = snapshot.panes.find(p => p.terminal_id === terminalId)!.pane_id
  const controller = start(["terminal", "session", "control", terminalId, "--cols", "100", "--rows", "30"])
  controller.stderr.resume()
  const frames = lines(controller.stdout)
  const send = (row: Row) => controller.stdin.write(JSON.stringify(row) + "\n")
  await frames.wait(row => row.type === "terminal.frame")
  const enter = windows ? "\r" : "\n"
  send({ type: "terminal.input", text: (windows
    ? "1..400 | ForEach-Object { Write-Output ('PROBE_ROW_' + $_) }"
    : "i=1; while [ $i -le 400 ]; do printf 'PROBE_ROW_%s\\n' \"$i\"; i=$((i+1)); done") + enter })
  type Pane = { pane: { scroll?: { max_offset_from_bottom: number } } }
  let info = await api<Pane>("pane.get", { pane_id: paneId })
  const historyDeadline = Date.now() + 15_000
  while ((info.pane.scroll?.max_offset_from_bottom ?? 0) < 300 && Date.now() < historyDeadline) {
    await sleep(100); info = await api<Pane>("pane.get", { pane_id: paneId })
  }
  check((info.pane.scroll?.max_offset_from_bottom ?? 0) >= 300, "fixture did not produce scrollback")
  const getSamples: number[] = [], scrollSamples: number[] = []
  for (let i = 0; i < iterations; i++) await timed(getSamples, () => api("pane.get", { pane_id: paneId }))
  for (let i = 0; i < iterations; i++) await timed(scrollSamples, () => api("pane.scroll", { pane_id: paneId, offset_from_bottom: i % 2 ? 50 : 100 }))
  await api("pane.scroll", { pane_id: paneId, offset_from_bottom: 0 })
  latency.paneGetMs = stats(getSamples)
  latency.paneScrollMs = stats(scrollSamples)

  const frameSamples: number[] = [], frameBytes: number[] = []
  let full = 0
  await sleep(300); frames.clear()
  for (let i = 0; i < 30; i++) {
    const at = performance.now()
    send({ type: "terminal.scroll", direction: i % 2 ? "down" : "up", lines: 3 })
    const frame = await frames.wait(row => row.type === "terminal.frame", 5_000)
    frameSamples.push(performance.now() - at)
    frameBytes.push(Math.floor(String(frame.bytes ?? "").length * 3 / 4))
    if (frame.full === true) full++
    await sleep(20); frames.clear()
  }
  latency.terminalScrollToFrameMs = stats(frameSamples)
  latency.terminalScrollFrames = { bytes: stats(frameBytes), full, total: frameSamples.length }
  console.log("Part A latency:", JSON.stringify(latency, null, 2))

  // Part B — which delivery reaches the pane program as the intended key.
  if (!flag("--skip-input")) {
    const runtime = hasCommand(windows ? "node.exe" : "node") ? "node" : process.execPath
    await writeFile(join(root, "reader.mjs"), `
const tag = process.argv[2], kitty = process.argv.includes("--kitty")
process.stdin.setRawMode(true); process.stdin.resume()
if (kitty) process.stdout.write("\\x1b[>1u")
process.stdout.write("READY " + tag + "\\r\\n")
const quit = () => { if (kitty) process.stdout.write("\\x1b[<u"); process.exit(0) }
setTimeout(quit, 120000)
process.stdin.on("data", d => {
  const hex = Buffer.from(d).toString("hex")
  process.stdout.write("RAW " + tag + " " + hex + "\\r\\n")
  if (hex === "03" || hex.endsWith("5b39393b3575")) quit()
})
`)
    await writeFile(join(root, "readkey.ps1"), `
[Console]::TreatControlCAsInput = $true
$tag = $args[0]
Write-Host ("READY " + $tag)
while ($true) {
  $k = [Console]::ReadKey($true)
  Write-Host ("KEY " + $tag + " vk=" + [int]$k.Key + " mods=" + [int]$k.Modifiers + " ch=" + [int]$k.KeyChar)
  if ([int]$k.KeyChar -eq 3) { break }
}
`)
    const quote = (value: string) => windows ? `'${value.replaceAll("'", "''")}'` : `'${value.replaceAll("'", "'\\''")}'`
    const programs = [
      ...(windows ? [{ id: "P1", launch: (tag: string) => `& ${quote(shell)} -NoProfile -ExecutionPolicy Bypass -File ${quote(join(root, "readkey.ps1"))} ${tag}` }] : []),
      { id: "P2", launch: (tag: string) => `${windows ? "& " : ""}${quote(runtime)} ${quote(join(root, "reader.mjs"))} ${tag}` },
      { id: "P3", launch: (tag: string) => `${windows ? "& " : ""}${quote(runtime)} ${quote(join(root, "reader.mjs"))} ${tag} --kitty` },
    ]
    // win32-input-mode: CSI Vk;Sc;Uc;Kd;Cs;Rc _ (press + release), LEFT_ALT = 0x2.
    const w32 = (vk: number, sc: number, uc: number, cs: number) =>
      `\x1b[${vk};${sc};${uc};1;${cs};1_\x1b[${vk};${sc};${uc};0;${cs};1_`
    const keys = [
      { name: "alt+q", raw: "\x1bq", w32: w32(81, 16, 113, 2), vk: 81, alt: true, legacy: "1b71", kitty: "1b5b3131333b3375" },
      { name: "alt+t", raw: "\x1bt", w32: w32(84, 20, 116, 2), vk: 84, alt: true, legacy: "1b74", kitty: "1b5b3131363b3375" },
      { name: "alt+enter", raw: "\x1b\r", w32: w32(13, 28, 13, 2), vk: 13, alt: true, legacy: "1b0d", kitty: "1b5b31333b3375" },
      { name: "esc", raw: "\x1b", w32: w32(27, 1, 27, 0), vk: 27, alt: false, legacy: "1b", kitty: "1b5b323775" },
    ]
    const tagged = async (tag: string) => {
      const read = await api<{ read: { text: string } }>("pane.read", { pane_id: paneId, source: "recent", lines: 120, format: "text" })
      return read.read.text.split(/\r?\n/).map(line => line.trim()).filter(line => line.includes(` ${tag}`) && /^(KEY|RAW|READY) /.test(line))
    }
    const classify = (program: string, key: typeof keys[number], observed: string[]): Verdict => {
      if (!observed.length) return "none"
      if (program === "P1") {
        const parsed = observed.map(line => Object.fromEntries([...line.matchAll(/(vk|mods)=(\d+)/g)].map(m => [m[1], Number(m[2])])))
        if (parsed.length === 1 && parsed[0].vk === key.vk && Boolean(parsed[0].mods & 1) === key.alt) return "ok"
        if (parsed.length === 2 && parsed[0].vk === 27 && parsed[1].vk === key.vk) return "split"
        return "other"
      }
      const chunks = observed.map(line => line.split(" ").at(-1) ?? "")
      if (chunks.length === 1 && (chunks[0] === key.legacy || chunks[0] === key.kitty)) return "ok"
      if (chunks.length >= 2 && chunks[0] === "1b" && key.name !== "esc") return "split"
      if (chunks.join("").includes("5f")) return "raw-w32"
      return "other"
    }
    let serial = 0
    for (const program of programs) {
      const tag = `t${++serial}${program.id}`
      send({ type: "terminal.input", text: program.launch(tag) + enter })
      const readyDeadline = Date.now() + 15_000
      while (!(await tagged(tag)).some(line => line.startsWith("READY")) && Date.now() < readyDeadline) await sleep(150)
      if (!(await tagged(tag)).some(line => line.startsWith("READY"))) {
        (result.input as unknown[]).push({ program: program.id, error: "program did not start" })
        continue
      }
      for (const key of keys) {
        const deliveries: Array<[string, () => Promise<unknown> | void]> = [
          ["D1", () => { send({ type: "terminal.input", text: key.raw }) }],
          ["D2", () => { send({ type: "terminal.input", bytes: Buffer.from(key.w32).toString("base64") }) }],
          ["D3", () => api("pane.send_keys", { pane_id: paneId, keys: [key.name] })],
        ]
        for (const [delivery, deliver] of deliveries) {
          const before = (await tagged(tag)).filter(line => !line.startsWith("READY")).length
          await deliver()
          await sleep(400)
          const observed = (await tagged(tag)).filter(line => !line.startsWith("READY")).slice(before)
          ;(result.input as unknown[]).push({ program: program.id, delivery, key: key.name, verdict: classify(program.id, key, observed), observed })
        }
      }
      send({ type: "terminal.input", text: "\x03" })
      await sleep(500)
    }
    const rows = result.input as Array<{ program: string; delivery?: string; key?: string; verdict?: string; error?: string }>
    console.log("Part B input matrix:")
    for (const row of rows) console.log(row.error ? `  ${row.program}: ${row.error}` : `  ${row.program} ${row.delivery} ${row.key?.padEnd(10)} ${row.verdict}`)
  }
  send({ type: "terminal.release" })
  const out = option("--out")
  if (out) { await writeFile(out, JSON.stringify(result, null, 2)); console.log(`wrote ${out}`) }
  console.log(`PROBE OK ${platform()} herdr ${(result.host as Row).herdr}`)
} finally {
  for (const socket of sockets) socket.destroy()
  for (const child of children) if (child !== server && child.exitCode === null) child.stdin.end()
  try { if (server) await command(["session", "stop", session, "--json"], false) }
  catch (error) { console.error(`session stop failed: ${String(error)}`) }
  finally {
    const waitForChildren = () => Promise.all(children.filter(child => child.exitCode === null && child.signalCode === null).map(child => new Promise<void>(resolve => {
      const timer = setTimeout(resolve, 3000)
      child.once("close", () => { clearTimeout(timer); resolve() })
    })))
    await waitForChildren()
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill()
    await waitForChildren()
    await removeRuntimeFixture(root)
  }
}
