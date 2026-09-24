import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdtemp, mkdir, readFile, writeFile, realpath } from "node:fs/promises"
import { join, relative, isAbsolute } from "node:path"
import { createConnection } from "node:net"
import { createInterface } from "node:readline"
import { Terminal } from "@xterm/xterm"
import { removeRuntimeFixture } from "./runtime-fixture-cleanup"

// Local acceptance of the production host PTY transport, not a mocked browser.
// All commands target our temporary config and named Session, including cleanup.
const binary = await realpath(process.argv[2] ?? "src-tauri/resources/herdr/macos-aarch64/herdr")
const helper = await realpath(process.argv[3] ?? "src-tauri/host/target/debug/yuzora-host")
const evidence = await realpath(await mkdtemp("/tmp/yuzora-native-e2e-evidence-"))
const root = await realpath(await mkdtemp("/tmp/yz-native-"))
const session = "native-e2e"
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("HERDR_") && !["ENV", "BASH_ENV", "KITTY_WINDOW_ID"].includes(key)))
Object.assign(env, {
  XDG_CONFIG_HOME: join(root, "cfg"), XDG_STATE_HOME: join(root, "state"),
  XDG_RUNTIME_DIR: join(root, "run"), HERDR_CONFIG_PATH: join(root, "config.toml"),
  HISTFILE: join(root, "shell-history"), SHELL: "/bin/sh", TERM: "xterm-256color",
  // Exercise the remote clipboard path without touching the user's clipboard.
  SSH_CONNECTION: "127.0.0.1 12345 127.0.0.1 22",
})
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))
const children: ChildProcessWithoutNullStreams[] = []
const assertions: string[] = []
const transcripts: Array<{ label: string; screen: string; raw: string }> = []
const activeClients: Array<{ screen: () => string; raw: () => string }> = []
const nativeClientPids: number[] = []
const owner = { hostId: "isolated-native-e2e", generation: 1 }
type Pane = { pane_id: string; terminal_id: string }
type Snapshot = { snapshot: { panes: Pane[] } }
type Status = { server: { running: boolean; compatible: boolean; socket: string; version: string; protocol: number } }
type Reply = { id: string; status: string; value: unknown }
type StreamRow = { payload: { type: string; id?: string; outcome: Reply; event: { type: string; bytesBase64: string } } }
let serial = 0
let server: ChildProcessWithoutNullStreams | undefined
let socketPath = ""
function check(value: unknown, message: string): asserts value { if (!value) throw new Error(message) }
function pass(message: string) { assertions.push(message); console.log(`PASS ${message}`) }
function start(executable: string, args: string[]) {
  const child = spawn(executable, args, { cwd: join(root, "work"), env, stdio: "pipe" })
  children.push(child)
  return child
}
async function exited(child: ChildProcessWithoutNullStreams, timeout = 10_000) {
  if (child.exitCode !== null || child.signalCode !== null) return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`process ${child.pid} did not exit`)), timeout)
    child.once("error", error => { clearTimeout(timer); reject(error) })
    child.once("close", () => { clearTimeout(timer); resolve() })
  })
}
async function cli<T = unknown>(args: string[]): Promise<T> {
  const child = start(binary, ["--session", session, ...args])
  let out = "", err = ""
  child.stdout.on("data", bytes => { out += bytes })
  child.stderr.on("data", bytes => { err += bytes })
  await exited(child)
  check(child.exitCode === 0, `${args.join(" ")}: ${err}`)
  return JSON.parse(out)
}
async function until(predicate: () => boolean | Promise<boolean>, message: string, timeout = 12_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) { if (await predicate()) return; await sleep(30) }
  throw new Error(`Timed out: ${message}`)
}
async function api<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  const socket = createConnection(socketPath)
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method}: timeout`)), 10_000)
      const reader = createInterface({ input: socket })
      reader.once("line", line => {
        clearTimeout(timer)
        const row = JSON.parse(line)
        if (row.error) reject(new Error(`${method}: ${JSON.stringify(row.error)}`))
        else resolve(row.result)
      })
      socket.once("error", error => { clearTimeout(timer); reject(error) })
      socket.once("connect", () => socket.write(JSON.stringify({ id: `api-${++serial}`, method, params }) + "\n"))
    })
  } finally { socket.destroy() }
}
function lane(flag: "--stream" | "--stdio") {
  const child = start(helper, [flag])
  const rows: Array<Reply | StreamRow> = []
  let stderr = ""
  child.stderr.on("data", bytes => { stderr += bytes })
  createInterface({ input: child.stdout }).on("line", line => rows.push(JSON.parse(line)))
  async function request<T = unknown>(operation: object): Promise<T> {
    const id = `host-${++serial}`
    child.stdin.write(JSON.stringify({ version: 1, owner, id, operation }) + "\n")
    await until(() => rows.some(row => ("payload" in row ? row.payload : row).id === id), `${flag} ${id}; ${stderr}`)
    const row = rows.find(row => ("payload" in row ? row.payload : row).id === id)!
    const outcome = "payload" in row ? row.payload.outcome : row
    check(outcome.status === "ok", JSON.stringify(outcome))
    return outcome.value as T
  }
  return { child, rows, request }
}
async function openClient() {
  const stream = lane("--stream")
  const terminal = new Terminal({ cols: 100, rows: 30, allowProposedApi: true })
  let raw = "", consumed = 0, writing = false
  const clipboard: string[] = []
  terminal.parser.registerOscHandler(52, data => { clipboard.push(Buffer.from(data.split(";").slice(1).join(";"), "base64").toString()); return true })
  terminal.onData(text => { void stream.request({ command: "input", text, bytesBase64: null }) })
  const opened = await stream.request<{ sessionId: string }>({ command: "open", config: { kind: "client", binary, sessionName: session, size: { cols: 100, rows: 30, cellWidth: 9, cellHeight: 18 } } })
  const lookup = start("/usr/bin/pgrep", ["-P", String(stream.child.pid)])
  let descendantPids = ""
  lookup.stdout.on("data", chunk => { descendantPids += chunk }); lookup.stderr.resume()
  await exited(lookup)
  const ownedPids = descendantPids.trim().split(/\s+/).filter(value => /^\d+$/.test(value)).map(Number)
  check(ownedPids.length === 1, "one official client process belongs to the native host stream")
  nativeClientPids.push(...ownedPids)
  const pump = setInterval(() => {
    if (writing) return
    const rows = stream.rows.slice(consumed)
    consumed = stream.rows.length
    const bytes = rows.flatMap(row => "payload" in row && row.payload.type === "terminal" && row.payload.event.type === "frame" ? [Buffer.from(row.payload.event.bytesBase64, "base64")] : [])
    if (!bytes.length) return
    const chunk = Buffer.concat(bytes)
    raw += chunk.toString()
    writing = true
    terminal.write(chunk, () => { writing = false })
  }, 10)
  function screen() {
    const buffer = terminal.buffer.active
    return Array.from({ length: terminal.rows }, (_, i) => buffer.getLine(buffer.viewportY + i)?.translateToString(true) ?? "").join("\n")
  }
  async function capture(label: string) {
    await sleep(120)
    transcripts.push({ label, screen: screen(), raw })
    await writeFile(join(evidence, `${label}.txt`), screen())
  }
  async function close() {
    await stream.request({ command: "close" })
    stream.child.stdin.end()
    try { await exited(stream.child) } catch (error) {
      if (process.platform === "darwin" && stream.child.pid) {
        const lookup = start("/usr/bin/pgrep", ["-P", String(stream.child.pid)])
        let descendantPids = ""
        lookup.stdout.on("data", chunk => { descendantPids += chunk })
        lookup.stderr.resume()
        await exited(lookup, 5000)
        await writeFile(join(evidence, "stalled-child-pids.txt"), descendantPids)
        for (const pid of descendantPids.trim().split(/\s+/).filter(value => /^\d+$/.test(value))) {
          const childSample = start("/usr/bin/sample", [pid, "1", "1", "-file", join(evidence, `stalled-child-${pid}.txt`)])
          childSample.stdout.resume(); childSample.stderr.resume()
          await exited(childSample, 5000).catch(() => childSample.kill())
        }
        const sample = start("/usr/bin/sample", [String(stream.child.pid), "1", "1", "-file", join(evidence, `stalled-host-${stream.child.pid}.txt`)])
        sample.stdout.resume(); sample.stderr.resume()
        await exited(sample, 5000).catch(() => sample.kill())
      }
      throw error
    }
    clearInterval(pump)
    for (const pid of ownedPids) {
      let alive = true
      try { process.kill(pid, 0) } catch (error) { if ((error as NodeJS.ErrnoException).code === "ESRCH") alive = false; else throw error }
      check(!alive, `native client ${pid} must be reaped when helper closes`)
    }
    transcripts.push({ label: `closed-${transcripts.length}`, screen: screen(), raw })
    activeClients.splice(activeClients.indexOf(client), 1)
    terminal.dispose()
    check(stream.child.exitCode === 0, "native host must release without an error")
  }
  const client = { ...stream, opened, terminal, screen, clipboard, capture, close, raw: () => raw,
    input: (text: string) => stream.request({ command: "input", text, bytesBase64: null }) }
  activeClients.push(client)
  return client
}

let failure: unknown
try {
  for (const name of ["cfg", "state", "run", "work", "plugin"]) await mkdir(join(root, name))
  await writeFile(join(root, "config.toml"), `onboarding = false
[remote]
manage_ssh_config = false
[terminal]
default_shell = "/bin/sh"
shell_mode = "non_login"
[session]
resume_agents_on_restore = false
[update]
version_check = false
manifest_check = false
`)
  await writeFile(join(root, "plugin", "herdr-plugin.toml"), `id = "yuzora.e2e"
name = "Yuzora isolated E2E"
version = "0.1.0"
min_herdr_version = "0.9.1"
platforms = ["macos", "linux"]
[[panes]]
id = "popup"
title = "E2E popup"
placement = "popup"
command = ${JSON.stringify(["/bin/sh", "-c", "printf 'YUZORA_POPUP_VISIBLE\\n'; read answer; printf '%s\\n' \"$answer\"; sleep 1"])}
[[panes]]
id = "board"
title = "E2E board"
placement = "tab"
command = ["/bin/sh", "-c", "printf 'YUZORA_PLUGIN_PANE_VISIBLE\\n'; sleep 120"]
`)
  server = start(binary, ["--session", session, "server"])
  server.stdout.resume(); server.stderr.resume()
  let status: Status | undefined
  await until(async () => { status = await cli<Status>(["status", "--json"]); return status.server?.running === true }, "isolated server start")
  check(status, "server status required")
  check(status.server.compatible === true, "server compatibility")
  socketPath = status.server.socket
  const marker = relative(root, socketPath)
  check(marker && !isAbsolute(marker) && marker !== ".." && !marker.startsWith("../"), "socket must stay inside temporary root")
  pass(`isolated HERDR ${status.server.version} protocol ${status.server.protocol} server`)
  const workspace = await api<{ root_pane: Pane; workspace: { workspace_id: string } }>("workspace.create", { cwd: join(root, "work"), label: "Native E2E", focus: true })
  const paneId = workspace.root_pane.pane_id
  const terminalId = workspace.root_pane.terminal_id
  const client = await openClient()
  check(client.opened.sessionId.startsWith("herdr-client-"), "production native client identity")
  await until(() => client.screen().includes("Native E2E"), "official client initial render")
  await client.input("printf 'YUZORA_%s\\n' 'NATIVE_INPUT_OK'\r")
  await until(() => client.screen().includes("YUZORA_NATIVE_INPUT_OK"), "native PTY keyboard reaches pane")
  pass("production native client open, ANSI frames and keyboard input")
  await client.capture("01-native-input")
  client.terminal.resize(112, 34)
  await client.request({ command: "resize", cols: 112, rows: 34 })
  await until(async () => (await api<{ pane: { scroll: { viewport_rows: number } } }>("pane.get", { pane_id: paneId })).pane.scroll.viewport_rows >= 30, "resize reaches pane layout")
  await client.input("printf 'SIZE_%s\\n' \"$(stty size)\"\r")
  await until(() => /SIZE_\d+ \d+/.test(client.screen()), "PTY resized pane")
  const resized = client.screen().match(/SIZE_(\d+) (\d+)/)!
  check(Number(resized[1]) >= 30 && Number(resized[2]) >= 80, "resize must reach the shell PTY geometry")
  await client.capture("02-resized")
  pass("native client resize acknowledged and shell remains interactive")
  await client.input("i=1; while [ $i -le 180 ]; do printf 'HISTORY_%03d\\n' \"$i\"; i=$((i+1)); done\r")
  await until(() => client.screen().includes("HISTORY_180"), "history fixture produced")
  await client.input("\x02[")
  await until(() => /COPY/i.test(client.screen()), "keyboard Copy mode")
  await client.input("/HISTORY_042\r")
  await until(() => client.screen().includes("HISTORY_042"), "history search result visible")
  await client.capture("03-copy-search")
  await client.input("y")
  await until(() => client.clipboard.includes("HISTORY_042"), "OSC 52 clipboard copy search match")
  pass("keyboard Copy mode searches offscreen history and emits exact OSC 52 text")
  const rgba = Buffer.alloc(16 * 16 * 4)
  for (let i = 0; i < rgba.length; i += 4) { rgba[i] = 255; rgba[i + 3] = 255 }
  const graphicsStart = client.raw().length
  await client.input(`printf '\\033_Ga=T,f=32,s=16,v=16,c=4,r=2,i=123,q=2;${rgba.toString("base64")}\\033\\\\'\r`)
  const escape = String.fromCharCode(27)
  const inlineGraphics = new RegExp(`${escape}_G[^;]*(?:a=[tT])[^;]*;[A-Za-z0-9+/=]+${escape}\\\\`)
  await until(() => inlineGraphics.test(client.raw().slice(graphicsStart)), "inline Kitty graphics data from official client")
  check(!new RegExp(`${escape}_G[^;]*t=[fs]`).test(client.raw()), "native client must not use inaccessible file graphics transport")
  await client.capture("04-kitty")
  pass("official client forwards inline Kitty graphics payload")
  await cli(["plugin", "link", join(root, "plugin")])
  const control = lane("--stdio")
  await control.request({ method: "hello" })
  async function feature<T = unknown>(method: string, params: object) {
    return control.request<T>({ method: "herdrCall", params: { binary, call: { command: "herdr_feature", args: { sessionName: session, request: { method, params } } } } })
  }
  const topologyBefore = (await api<Snapshot>("session.snapshot")).snapshot.panes.map(p => p.pane_id)
  await feature("plugin.pane.open", { plugin_id: "yuzora.e2e", entrypoint: "popup", placement: "popup", focus: true })
  await until(() => client.screen().includes("YUZORA_POPUP_VISIBLE"), "plugin popup visible through native client")
  await client.capture("05-plugin-popup")
  check(JSON.stringify((await api<Snapshot>("session.snapshot")).snapshot.panes.map(p => p.pane_id)) === JSON.stringify(topologyBefore), "popup must not change pane topology")
  await client.input("POPUP_INPUT\r")
  await until(() => !client.screen().includes("YUZORA_POPUP_VISIBLE"), "popup exits after keyboard input")
  pass("production feature opens native plugin popup; keyboard works and pane topology is unchanged")
  const pluginOpened = await feature<{ plugin_pane: { pane: Pane } }>("plugin.pane.open", { plugin_id: "yuzora.e2e", entrypoint: "board", placement: "tab", workspace_id: workspace.workspace.workspace_id, focus: true })
  await api("pane.focus", { pane_id: pluginOpened.plugin_pane.pane.pane_id })
  await until(() => client.screen().includes("YUZORA_PLUGIN_PANE_VISIBLE"), "plugin pane render")
  await client.capture("06-plugin-pane")
  pass("production feature opens and renders trusted local plugin pane")
  await api("pane.focus", { pane_id: paneId })
  await until(() => client.screen().includes("HISTORY_180"), "original pane focus reaches native client")
  await client.input("printf '%s\\n' $$ > pane.pid; printf 'ALIVE_%s\\n' 'BEFORE_RELEASE'\r")
  await until(() => client.screen().includes("ALIVE_BEFORE_RELEASE"), "pane process fixture ready")
  const panePid = Number((await readFile(join(root, "work", "pane.pid"), "utf8")).trim())
  check(Number.isInteger(panePid) && panePid > 1, "test shell process id")
  const before = await api<{ pane: Pane }>("pane.get", { pane_id: paneId })
  await client.close()
  check((await cli<Status>(["status", "--json"])).server.running === true, "closing client must preserve server")
  check((await api<{ pane: Pane }>("pane.get", { pane_id: paneId })).pane.terminal_id === before.pane.terminal_id, "closing client must preserve pane process identity")
  process.kill(panePid, 0)
  pass("client close reaps owned helper and preserves server plus pane identity")
  const connector = lane("--stream")
  const opened = await connector.request<{ target: string }>({ command: "open", config: { kind: "terminal", binary, sessionName: session, target: terminalId, mode: "control", takeover: false, cols: 100, rows: 30 } })
  check(opened.target === terminalId, "ordinary connector reconnect target")
  await connector.request({ command: "input", text: "printf 'RECONNECTED_%s\\n' 'OK'\r", bytesBase64: null })
  await until(async () => JSON.stringify(await api("pane.read", { pane_id: paneId, source: "recent_unwrapped", lines: 30 })).includes("RECONNECTED_OK"), "ordinary pane connector after native release")
  await connector.request({ command: "close" }); connector.child.stdin.end(); await exited(connector.child)
  pass("ordinary pane connector reconnects and delivers input after native client closes")
  const reopened = await openClient()
  await until(() => reopened.screen().includes("RECONNECTED_OK"), "native client reopen retains same pane output")
  await reopened.capture("07-reopened")
  await reopened.close()
  pass("native client can reopen same running Session")
  control.child.stdin.end(); await exited(control.child)
} catch (error) {
  failure = error
  console.error(error)
} finally {
  for (const [index, client] of activeClients.entries()) transcripts.push({ label: `final-client-${index}`, screen: client.screen(), raw: client.raw() })
  for (const child of children) if (child !== server && child.exitCode === null && child.signalCode === null) child.stdin.end()
  try { if (server) await cli(["session", "stop", session, "--json"]) } catch (error) { failure ??= error }
  for (const child of children) {
    try { await exited(child, 3000) } catch { child.kill(); await exited(child, 3000).catch(() => {}) }
  }
  await writeFile(join(evidence, "transcripts.json"), JSON.stringify(transcripts, null, 2))
  await writeFile(join(evidence, "result.json"), JSON.stringify({ passed: !failure, assertions, failure: failure ? String(failure) : null, nativeClientPids, root, session, cleanup: "only created test Session stopped; temporary runtime root removed" }, null, 2))
  await removeRuntimeFixture(root)
  console.log(`Evidence: ${evidence}`)
  // xterm's unmounted browser implementation retains internal scheduling timers.
  process.exit(failure ? 1 : 0)
}
