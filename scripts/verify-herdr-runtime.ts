import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdtemp, mkdir, writeFile, realpath, rm } from "node:fs/promises"
import { join, relative, isAbsolute } from "node:path"
import { tmpdir } from "node:os"
import { createConnection, type Socket } from "node:net"
import { createInterface } from "node:readline"
import { HERDR_RESOURCE_VERSION } from "./prepare-herdr-resources"
import methodFixture from "../src-tauri/host/tests/fixtures/herdr-0.9.0-methods.json"

// Uses only temporary XDG roots and its own named server. Never stops a user's server.
check(process.argv[2], "usage: bun scripts/verify-herdr-runtime.ts /absolute/path/to/herdr")
const binary = await realpath(process.argv[2])
// Keep Unix socket paths below macOS's length limit, including the session suffix.
const windows = process.platform === "win32"
const shell = windows ? "pwsh.exe" : "/bin/sh"
const root = await realpath(await mkdtemp(join(windows ? tmpdir() : "/tmp", "yz-h9-")))
const session = "contract-smoke"
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("HERDR_") && !["ENV", "BASH_ENV"].includes(key)))
Object.assign(env, { XDG_CONFIG_HOME: join(root, "cfg"), XDG_STATE_HOME: join(root, "state"), XDG_RUNTIME_DIR: join(root, "run"), HERDR_CONFIG_PATH: join(root, "config.toml"), HISTFILE: join(root, "shell-history"), SHELL: shell })
if (windows) Object.assign(env, { APPDATA: join(root, "cfg"), LOCALAPPDATA: join(root, "state") })
const children: ChildProcessWithoutNullStreams[] = []
const sockets: Socket[] = []
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))
function check(ok: unknown, message: string): asserts ok { if (!ok) throw new Error(message) }
function start(args: string[]) {
  const child = spawn(binary, ["--session", session, ...args], { cwd: join(root, "work"), env, stdio: "pipe" })
  children.push(child)
  return child
}
async function command(args: string[]) {
  const child = start(args)
  let output = "", error = ""
  child.stdout.on("data", chunk => { output += chunk; if (output.length > 2_000_000) child.kill() })
  child.stderr.on("data", chunk => { error += chunk; if (error.length > 2_000_000) child.kill() })
  const timer = setTimeout(() => child.kill(), 10_000)
  try {
    await new Promise<void>((resolve, reject) => {
      child.once("error", reject)
      child.once("close", code => code === 0 ? resolve() : reject(new Error(`${args.join(" ")}: ${error.slice(-1000)}`)))
    })
    return JSON.parse(output)
  } finally { clearTimeout(timer) }
}
function collect(input: NodeJS.ReadableStream) {
  const rows: Record<string, unknown>[] = []
  let failure: Error | null = null
  const reader = createInterface({ input })
  reader.on("line", line => {
    try {
      if (line.length > 2_000_000 || rows.length >= 2000) throw new Error("runtime output limit")
      rows.push(JSON.parse(line))
    } catch (error) { failure = error as Error; reader.close() }
  })
  input.on("error", error => { failure = error })
  return async (accept: (row: Record<string, unknown>) => boolean) => {
    const deadline = Date.now() + 10_000
    while (Date.now() < deadline) {
      if (failure) throw failure
      const row = rows.find(accept)
      if (row) return row
      await sleep(20)
    }
    throw new Error(`runtime response timeout; observed types: ${rows.map(r => r.type ?? r.event ?? r.id).join(", ")}`)
  }
}
let server: ChildProcessWithoutNullStreams | undefined
try {
  for (const name of ["cfg", "state", "run", "work"]) await mkdir(join(root, name))
  await writeFile(join(root, "config.toml"), `onboarding = false
[remote]
manage_ssh_config = false
[terminal]
default_shell = "${shell}"
shell_mode = "non_login"
[session]
resume_agents_on_restore = false
[update]
version_check = false
manifest_check = false
`)
  const schema = await command(["api", "schema", "--json"])
  check(schema.protocol === HERDR_RESOURCE_VERSION.protocol, "unexpected official schema protocol")
  const methods = schema.schemas.request.oneOf.map((entry: { properties: { method: { const: string } } }) => entry.properties.method.const)
  check(JSON.stringify(methods) === JSON.stringify(methodFixture.methods), "official method fixture must match the pinned binary")
  console.log(`Verified HERDR ${HERDR_RESOURCE_VERSION.baseVersion} protocol ${schema.protocol} method schema`)
  server = start(["server"])
  server.stdout.resume()
  server.stderr.resume()
  let status
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    status = await command(["status", "--json"])
    if (status.server?.running) break
    await sleep(100)
  }
  check(status?.server?.compatible === true, "isolated server must match its client")
  check(status.client.version === HERDR_RESOURCE_VERSION.baseVersion, "unexpected client version")
  check(status.server.version === HERDR_RESOURCE_VERSION.baseVersion, "unexpected server version")
  check(status.client.protocol === HERDR_RESOURCE_VERSION.protocol && status.server.protocol === HERDR_RESOURCE_VERSION.protocol, "status protocol mismatch")
  check(typeof status.server.socket === "string", "missing isolated socket marker")
  const socketRelative = relative(root, status.server.socket)
  check(socketRelative.length > 0 && !isAbsolute(socketRelative) && socketRelative !== ".." && !socketRelative.startsWith("..\\") && !socketRelative.startsWith("../"), "refuse a socket outside the isolated test root")
  console.log(`Started isolated Session ${session} at ${status.server.socket}`)
  const socketPath = windows ? "\\\\.\\pipe\\" + status.server.socket : status.server.socket
  async function connect() {
    const socket = createConnection(socketPath)
    sockets.push(socket)
    const wait = collect(socket)
    await new Promise<void>((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject) })
    return { socket, wait }
  }
  // events.subscribe owns a streaming connection; ordinary requests use their own sockets.
  const events = await connect()
  let id = 0
  async function api<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const requestId = `smoke-${++id}`
    const connection = method === "events.subscribe" ? events : await connect()
    connection.socket.write(JSON.stringify({ id: requestId, method, params }) + "\n")
    const response = await connection.wait(row => row.id === requestId)
    if (connection !== events) connection.socket.destroy()
    check(!response.error, `${method}: ${JSON.stringify(response.error)}`)
    return response.result as T
  }
  const subscription = await api<{ type: string }>("events.subscribe", { subscriptions: [{ type: "workspace.created" }, { type: "tab.created" }] })
  check(subscription.type === "subscription_started", "subscription acknowledgement contract changed")
  type Snapshot = { protocol: number; panes: Array<{ terminal_id?: string }> }
  const initial = await api<{ snapshot: Snapshot }>("session.snapshot")
  check(initial.snapshot.protocol === HERDR_RESOURCE_VERSION.protocol, "snapshot protocol mismatch")
  const created = await api<{ root_pane: { terminal_id: string } }>("workspace.create", { cwd: join(root, "work"), label: "Yuzora runtime smoke", focus: true })
  await events.wait(row => row.event === "workspace_created")
  console.log("Verified subscription acknowledgement, initial snapshot and live workspace event")
  const snapshot = (await api<{ snapshot: Snapshot }>("session.snapshot")).snapshot
  const pane = snapshot.panes.find(pane => pane.terminal_id === created.root_pane.terminal_id)
  check(pane?.terminal_id, "snapshot must expose terminal identity")
  const observer = start(["terminal", "session", "observe", pane.terminal_id, "--cols", "80", "--rows", "24"])
  observer.stderr.resume()
  const observe = collect(observer.stdout)
  const first = await observe(row => row.type === "terminal.frame")
  check(first.full === true && typeof first.seq === "number" && typeof first.bytes === "string", "official observer frame contract changed")
  const controller = start(["terminal", "session", "control", pane.terminal_id, "--cols", "80", "--rows", "24"])
  controller.stderr.resume()
  const control = collect(controller.stdout)
  await control(row => row.type === "terminal.frame")
  const input = windows ? "Write-Output ('YUZORA_' + 'RUNTIME_OK')\r" : "printf 'YUZORA_%s\\n' 'RUNTIME_OK'\n"
  controller.stdin.write(JSON.stringify({ type: "terminal.input", text: input }) + "\n")
  await control(row => row.type === "terminal.frame" && typeof row.bytes === "string" && Buffer.from(row.bytes, "base64").toString().includes("YUZORA_RUNTIME_OK"))
  controller.stdin.write(JSON.stringify({ type: "terminal.resize", cols: 100, rows: 30 }) + "\n")
  await control(row => row.type === "terminal.frame" && row.width === 100 && row.height === 30)
  controller.stdin.write(JSON.stringify({ type: "terminal.release" }) + "\n")
  console.log(`HERDR ${HERDR_RESOURCE_VERSION.baseVersion} protocol ${schema.protocol}: snapshot, live events, observer, controller, input and resize passed`)
} finally {
  for (const socket of sockets) socket.destroy()
  for (const child of children) if (child !== server && child.exitCode === null) child.stdin.end()
  try { if (server) await command(["session", "stop", session, "--json"]) }
  finally {
    for (const child of children) if (child.exitCode === null) child.kill()
    // Windows keeps executable and working-directory handles until process exit.
    await Promise.all(children.filter(child => child.exitCode === null && child.signalCode === null).map(child => new Promise<void>(resolve => {
      const timer = setTimeout(resolve, 3000)
      child.once("close", () => { clearTimeout(timer); resolve() })
    })))
    await rm(root, { recursive: true, force: true })
  }
}
