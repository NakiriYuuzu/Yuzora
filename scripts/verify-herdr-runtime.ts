import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdtemp, mkdir, writeFile, realpath } from "node:fs/promises"
import { join, relative, isAbsolute } from "node:path"
import { tmpdir } from "node:os"
import { createConnection, type Socket } from "node:net"
import { createInterface } from "node:readline"
import { HERDR_RESOURCE_VERSION } from "./prepare-herdr-resources"
import { removeRuntimeFixture } from "./runtime-fixture-cleanup"
import methodFixture from "../src-tauri/host/tests/fixtures/herdr-0.9.0-methods.json"

// Uses only temporary XDG roots and its own named server. Never stops a user's server.
check(process.argv[2], "usage: bun scripts/verify-herdr-runtime.ts /absolute/path/to/herdr")
const binary = await realpath(process.argv[2])
const expectedVersion = process.argv[3] ?? HERDR_RESOURCE_VERSION.baseVersion
check(["0.8.2", "0.9.0"].includes(expectedVersion), "unverified runtime version; update compatibility fixtures first")
const expectedProtocol = expectedVersion === "0.8.2" ? 20 : 22
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
  check(schema.protocol === expectedProtocol, "unexpected official schema protocol")
  const methods = schema.schemas.request.oneOf.map((entry: { properties: { method: { const: string } } }) => entry.properties.method.const)
  if (expectedVersion === "0.9.0") check(JSON.stringify(methods) === JSON.stringify(methodFixture.methods), "official method fixture must match the pinned binary")
  console.log(`Verified HERDR ${expectedVersion} protocol ${schema.protocol} method schema`)
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
  check(status.client.version === expectedVersion, "unexpected client version")
  check(status.server.version === expectedVersion, "unexpected server version")
  check(status.client.protocol === expectedProtocol && status.server.protocol === expectedProtocol, "status protocol mismatch")
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
  check(initial.snapshot.protocol === expectedProtocol, "snapshot protocol mismatch")
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
  type WorkspaceList = { workspaces: Array<{ workspace_id: string }> }
  const beforeOrder = (await api<WorkspaceList>("workspace.list")).workspaces.map(w => w.workspace_id)
  const reorderA = await api<{ workspace: { workspace_id: string } }>("workspace.create", { cwd: join(root, "work"), label: "Reorder A", focus: false })
  const reorderB = await api<{ workspace: { workspace_id: string } }>("workspace.create", { cwd: join(root, "work"), label: "Reorder B", focus: false })
  const a = reorderA.workspace.workspace_id, b = reorderB.workspace.workspace_id
  const ids = (result: WorkspaceList) => result.workspaces.map(w => w.workspace_id)
  if (methods.includes("workspace.move")) {
    const moved = await api<WorkspaceList>("workspace.move", { workspace_id: a, insert_index: beforeOrder.length + 2 })
    check(JSON.stringify(ids(moved)) === JSON.stringify([...beforeOrder, b, a]), "downward legacy insertion boundary mismatch")
    console.log("PASS downward workspace.move authoritative order")
  } else console.log("NOT RUN workspace.move: method unavailable on this runtime")
  if (methods.includes("workspace.move_block")) {
    const moved = await api<WorkspaceList>("workspace.move_block", { workspace_ids: [a, b], before_workspace_id: beforeOrder[0] })
    check(JSON.stringify(ids(moved)) === JSON.stringify([a, b, ...beforeOrder]), "block reorder acknowledgement mismatch")
    console.log("PASS workspace.move_block authoritative order")
  } else console.log("NOT RUN workspace.move_block: method unavailable on this runtime")
  if (methods.includes("pane.get") && methods.includes("pane.scroll")) {
    const paneSnapshot = (await api<{ snapshot: { panes: Array<{ pane_id: string; terminal_id: string }> } }>("session.snapshot")).snapshot
    const paneId = paneSnapshot.panes.find(p => p.terminal_id === pane.terminal_id)!.pane_id
    const output = windows ? "1..400 | ForEach-Object { Write-Output ('SCROLL_ROW_' + $_) }\r" : "i=1; while [ $i -le 400 ]; do printf 'SCROLL_ROW_%s\\n' \"$i\"; i=$((i+1)); done\n"
    controller.stdin.write(JSON.stringify({ type: "terminal.input", text: output }) + "\n")
    type Scroll = { pane: { pane_id: string; scroll: { offset_from_bottom: number; max_offset_from_bottom: number; viewport_rows: number } } }
    // Connector frames are ANSI deltas. The final line may update only its
    // numeric suffix, so a substring search in one frame is not an output oracle.
    let info = await api<Scroll>("pane.get", { pane_id: paneId })
    const historyDeadline = Date.now() + 10000
    while ((info.pane.scroll?.max_offset_from_bottom ?? 0) < 300 && Date.now() < historyDeadline) {
      await sleep(100)
      info = await api<Scroll>("pane.get", { pane_id: paneId })
    }
    check(info.pane.scroll?.max_offset_from_bottom >= 300, "fixture did not produce the required real scrollback")
    const moved = await api<Scroll>("pane.scroll", { pane_id: paneId, offset_from_bottom: 100 })
    check(moved.pane.pane_id === paneId && moved.pane.scroll.offset_from_bottom === 100, "pane.scroll authoritative range mismatch")
    console.log("PASS pane.get/pane.scroll real history and exact pane identity")
  } else console.log("NOT RUN pane scroll metadata: method unavailable on this runtime")
  controller.stdin.write(JSON.stringify({ type: "terminal.release" }) + "\n")
  console.log(`HERDR ${expectedVersion} protocol ${schema.protocol}: snapshot, live events, observer, controller, input and resize passed`)
} finally {
  for (const socket of sockets) socket.destroy()
  for (const child of children) if (child !== server && child.exitCode === null) child.stdin.end()
  try { if (server) await command(["session", "stop", session, "--json"]) }
  finally {
    const waitForChildren = () => Promise.all(children.filter(child => child.exitCode === null && child.signalCode === null).map(child => new Promise<void>(resolve => {
      const timer = setTimeout(resolve, 3000)
      child.once("close", () => { clearTimeout(timer); resolve() })
    })))
    // The stop response can precede server/ConPTY teardown. Let the server
    // release its shell before killing any tracked process that remains alive.
    await waitForChildren()
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill()
    await waitForChildren()
    // Windows may release directory handles after the process exit event.
    // Retry transient filesystem locks for at most 5.5s; never ignore failure.
    await removeRuntimeFixture(root)
  }
}
