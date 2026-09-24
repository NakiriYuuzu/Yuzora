import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdtemp, mkdir, writeFile, realpath, readFile, access, chmod } from "node:fs/promises"
import { join, relative, isAbsolute } from "node:path"
import { createInterface } from "node:readline"
import type { HerdrCapabilities, HerdrPaneIdentity, HerdrNamedSession, HerdrWorkspaceCreateResult } from "../src/lib/herdrTypes"

type Response = { status: "ok"; value: unknown } | { status: "error"; message: string }
type FeatureWorktree = { workspace: { workspace_id: string } }
type FeatureMove = { move_result: { previous_pane_id: string; pane: { pane_id: string } } }
type Integrations = { integrations: { target: string; state: string }[] }
type RawSnapshot = { snapshot: { panes: { pane_id: string }[]; workspaces: { label: string }[] } }


// Explicit opt-in executable E2E. All subprocess homes/config, Sessions and git writes
// belong to the unique temporary fixture; never inherits an active HERDR socket.
const binary = await realpath(process.argv[2] ?? "src-tauri/resources/herdr/macos-aarch64/herdr")
const helper = await realpath(process.argv[3] ?? "src-tauri/host/target/debug/yuzora-host")
const root = await realpath(await mkdtemp("/tmp/yz-fe-"))
const session = "feature-e2e"
const repo = join(root, "repo")
const env: NodeJS.ProcessEnv = {
  PATH: `${join(root, "bin")}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: join(root, "home"),
  XDG_CONFIG_HOME: join(root, "cfg"), XDG_STATE_HOME: join(root, "state"),
  XDG_DATA_HOME: join(root, "data"), XDG_RUNTIME_DIR: join(root, "run"),
  HERDR_CONFIG_PATH: join(root, "config.toml"), HISTFILE: join(root, "history"),
  SHELL: "/bin/sh", TERM: "xterm-256color", LC_ALL: "en_US.UTF-8",
  GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1",
}
const transcript: unknown[] = []
const passes: string[] = []
let host: ChildProcessWithoutNullStreams | undefined
let hostError = ""
let id = 0
const pending = new Map<string, { resolve: (value: Response) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>()
function check(ok: unknown, message: string): asserts ok { if (!ok) throw new Error(message) }
function pass(message: string) { passes.push(message); console.log(`PASS ${message}`) }
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))
function contained(path: string) { const rel = relative(root, path); return rel !== "" && !isAbsolute(rel) && rel !== ".." && !rel.startsWith("../") }
async function run(executable: string, args: string[], cwd = root) {
  const child = spawn(executable, args, { cwd, env, stdio: "pipe" })
  let stdout = "", stderr = ""
  child.stdout.on("data", chunk => { stdout += chunk; if (stdout.length > 4_000_000) child.kill() })
  child.stderr.on("data", chunk => { stderr += chunk; if (stderr.length > 4_000_000) child.kill() })
  const timer = setTimeout(() => child.kill(), 20_000)
  try {
    const code = await new Promise<number | null>((resolve, reject) => { child.once("close", resolve); child.once("error", reject) })
    check(code === 0, `${executable} ${args.join(" ")} failed: ${stderr}\n${stdout}`)
    return stdout
  } finally { clearTimeout(timer) }
}
async function cli(args: string[]) { return JSON.parse(await run(binary, ["--session", session, ...args])) }
// The Agent Inspector IPC is gone; read agent state through Herdr's public CLI instead.
type AgentInfo = { result?: { agent?: { interactive_ready?: boolean; launch_pending?: boolean } } }
async function agentGet(target: string): Promise<{ agent: AgentInfo | null; error: string | null }> {
  try { return { agent: await cli(["agent", "get", target]) as AgentInfo, error: null } }
  catch (cause) { return { agent: null, error: String(cause) } }
}
async function request(operation: Record<string, unknown>) {
  check(host, "helper not running")
  const requestId = `feature-${++id}`
  const payload = { version: 1, id: requestId, owner: { hostId: "herdr-feature-e2e", generation: 1 }, operation }
  transcript.push({ request: payload })
  const response = await new Promise<Response>((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(requestId); reject(new Error(`request timeout ${requestId}: ${hostError}`)) }, 145_000)
    pending.set(requestId, { resolve, reject, timer })
    host!.stdin.write(JSON.stringify(payload) + "\n")
  })
  transcript.push({ response })
  return response
}
async function call<T = unknown>(command: string, args?: Record<string, unknown>) {
  const response = await request({ method: "herdrCall", params: { binary, call: args ? { command, args } : { command } } })
  check(response.status === "ok", `${command}: ${response.status === "error" ? response.message : "unknown error"}`)
  return response.value as T
}
async function feature<T = unknown>(method: string, params: Record<string, unknown> = {}, name = session) {
  return call<T>("herdr_feature", { sessionName: name, request: { method, params } })
}
let failure: string | undefined
try {
  for (const dir of ["home", "cfg", "state", "data", "run", "repo", "bin"]) await mkdir(join(root, dir))
  for (const dir of [".claude", ".codex"]) await mkdir(join(root, "home", dir))
  const shellQuote = (value: string) => "'" + value.replaceAll("'", "'\"'\"'") + "'"
  await writeFile(join(root, "bin", "pi"), `#!/bin/sh
export HERDR_AGENT=pi
${shellQuote(binary)} pane report-agent "$HERDR_PANE_ID" --source custom:yuzora-e2e --agent pi --state idle >/dev/null
while IFS= read -r prompt; do
  ${shellQuote(binary)} pane report-agent "$HERDR_PANE_ID" --source custom:yuzora-e2e --agent pi --state working >/dev/null
  printf '%s\\n' "$prompt" >> ${shellQuote(join(root, "agent-prompts.txt"))}
  /bin/sleep 0.15
  ${shellQuote(binary)} pane report-agent "$HERDR_PANE_ID" --source custom:yuzora-e2e --agent pi --state idle >/dev/null
done
`)
  await chmod(join(root, "bin", "pi"), 0o755)
  await writeFile(join(root, "home", ".claude", "settings.json"), JSON.stringify({ fixtureSetting: "preserve" }))
  await writeFile(join(root, "home", ".codex", "config.toml"), 'model = "e2e-no-provider"\n')
  await writeFile(join(root, "config.toml"), `onboarding = false\n[remote]\nmanage_ssh_config = false\n[terminal]\ndefault_shell = "/bin/sh"\nshell_mode = "non_login"\n[session]\nresume_agents_on_restore = false\n[update]\nversion_check = false\nmanifest_check = false\n`)
  await run("/usr/bin/git", ["init", "-b", "main"], repo)
  await writeFile(join(repo, "fixture.txt"), "HERDR management E2E fixture\n")
  await run("/usr/bin/git", ["add", "fixture.txt"], repo)
  await run("/usr/bin/git", ["-c", "user.name=E2E Fixture", "-c", "user.email=e2e@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", "fixture"], repo)
  host = spawn(helper, ["--stdio"], { cwd: root, env, stdio: "pipe" })
  host.stderr.on("data", chunk => { hostError += chunk })
  createInterface({ input: host.stdout }).on("line", line => {
    try {
      const row = JSON.parse(line)
      const waiter = pending.get(row.id)
      if (waiter) { clearTimeout(waiter.timer); pending.delete(row.id); waiter.resolve(row) }
    } catch (error) { for (const waiter of pending.values()) waiter.reject(error as Error) }
  })
  host.on("exit", code => { for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error(`helper exited ${code}: ${hostError}`)) }; pending.clear() })
  const hello = await request({ method: "hello" })
  check(hello.status === "ok" && contained((hello.value as { home: string }).home), "helper HOME must be isolated")
  const initial = await call<HerdrNamedSession[]>("herdr_sessions")
  check(initial.every((value) => value.name === "default" && !value.running && !!value.sessionDir && !!value.socketPath && contained(value.sessionDir) && contained(value.socketPath)), `isolated inventory must contain only stopped default placeholder: ${JSON.stringify(initial)}`)
  pass("isolated HOME/XDG and no running or saved user Sessions")

  const createdSession = await feature<{ started: boolean }>("session.start")
  check(createdSession.started === true, "new Session must start")
  const status = await cli(["status", "--json"])
  check(status.client.version === "0.9.1" && status.server.compatible === true, "running bundled v0.9.1 required")
  check(contained(status.server.socket), "runtime socket escaped temporary root")
  const repeat = await feature<{ started: boolean }>("session.start")
  check(repeat.started === false, "start must be idempotent")
  pass("Session create/start and repeated start; compatible protocol 22 socket stays isolated")

  const workspace = await call<HerdrWorkspaceCreateResult>("herdr_workspace_create", { sessionName: session, cwd: repo, label: "Management E2E", focus: false })
  transcript.push({ createdWorkspace: workspace })
  const workspaceId = workspace.workspaceId
  const paneId = workspace.paneId
  check(workspaceId && paneId, "workspace creation must return opaque IDs")
  const caps = await call<HerdrCapabilities>("herdr_capabilities", { sessionName: session })
  for (const method of ["worktree.create", "worktree.open", "worktree.remove", "pane.move", "integration.list", "integration.install", "integration.uninstall"]) check(caps.api.methods.includes(method), `missing capability ${method}`)
  pass("host capability gate advertises management operations")

  const worktreePath = join(root, "feature-worktree")
  const worktree = await feature<FeatureWorktree>("worktree.create", { workspace_id: workspaceId, branch: "e2e-branch", base: "HEAD", path: worktreePath, label: "E2E worktree", focus: false })
  transcript.push({ createdWorktree: worktree })
  check(contained(worktreePath), "unsafe worktree fixture path")
  check((await readFile(join(worktreePath, "fixture.txt"), "utf8")).includes("E2E fixture"), "worktree must contain fixture")
  const worktreeWorkspaceId = worktree.workspace.workspace_id
  check(worktreeWorkspaceId, "worktree must return workspace ID")
  await call("herdr_workspace_close", { sessionName: session, workspaceId: worktreeWorkspaceId })
  const reopened = await feature<FeatureWorktree>("worktree.open", { workspace_id: workspaceId, path: worktreePath, focus: false })
  check(reopened.workspace.workspace_id, "opening worktree must return workspace")
  await feature("worktree.remove", { workspace_id: reopened.workspace.workspace_id, force: false })
  check(!await access(worktreePath).then(() => true, () => false), "removed worktree path must disappear")
  const dirtyPath = join(root, "dirty-worktree")
  const dirty = await feature<FeatureWorktree>("worktree.create", { workspace_id: workspaceId, branch: "e2e-dirty", path: dirtyPath, focus: false })
  await writeFile(join(dirtyPath, "untracked.txt"), "do not silently discard\n")
  const refusedRemoval = await request({ method: "herdrCall", params: { binary, call: { command: "herdr_feature", args: { sessionName: session, request: { method: "worktree.remove", params: { workspace_id: dirty.workspace.workspace_id, force: false } } } } } })
  check(refusedRemoval.status === "error" && await access(join(dirtyPath, "untracked.txt")).then(() => true, () => false), "dirty worktree must not be removed without force")
  await feature("worktree.remove", { workspace_id: dirty.workspace.workspace_id, force: true })
  check(!await access(dirtyPath).then(() => true, () => false), "force removal must delete only fixture worktree")
  pass("real Git worktree create, close/open, remove, dirty refusal and explicit force through host boundary")

  const split = await call<HerdrPaneIdentity>("herdr_pane_split", { sessionName: session, direction: "right", targetPaneId: paneId, cwd: repo, focus: false })
  const sourcePaneId = split.paneId
  check(sourcePaneId, "split pane ID required")
  const move = await feature<FeatureMove>("pane.move", { pane_id: sourcePaneId, destination: { type: "new_workspace", label: "Moved E2E", tab_label: "Destination" }, focus: false })
  const movedId = move.move_result.pane.pane_id
  check(move.move_result.previous_pane_id === sourcePaneId && movedId && movedId !== sourcePaneId, "cross-workspace move must return replaced ID")
  const snapshot = await call<RawSnapshot>("herdr_snapshot", { sessionName: session })
  check(snapshot.snapshot.panes.some((pane) => pane.pane_id === movedId) && !snapshot.snapshot.panes.some((pane) => pane.pane_id === sourcePaneId), "snapshot must resolve only destination ID")
  const moveToTab = await feature<FeatureMove>("pane.move", { pane_id: movedId, destination: { type: "tab", tab_id: workspace.tabId, target_pane_id: paneId, split: "right" }, focus: false })
  const returnedId = moveToTab.move_result.pane.pane_id
  check(returnedId !== movedId && moveToTab.move_result.previous_pane_id === movedId, "move to existing tab must replace cross-workspace ID")
  const moveNewTab = await feature<FeatureMove>("pane.move", { pane_id: returnedId, destination: { type: "new_tab", workspace_id: workspaceId, label: "Moved new tab" }, focus: false })
  check(moveNewTab.move_result.pane.pane_id === returnedId, "same-workspace new-tab move must keep pane identity")
  pass("pane moves to new workspace, existing tab and new tab preserve authoritative destination identities")

  const integrations = await feature<Integrations>("integration.list")
  transcript.push({ integrations })
  for (const target of ["claude", "codex"]) {
    const item = integrations.integrations.find((value) => value.target === target)
    check(item, `integration ${target} not listed`)
    await feature("integration.install", { target })
    const installed = await feature<Integrations>("integration.list")
    check(installed.integrations.find((value) => value.target === target)?.state === "current", `${target} should be current after install`)
    const hookPath = join(root, "home", target === "claude" ? ".claude/hooks/herdr-agent-state.sh" : ".codex/herdr-agent-state.sh")
    await writeFile(hookPath, "#!/bin/sh\n# deliberately stale E2E fixture\n")
    const outdated = await feature<Integrations>("integration.list")
    check(outdated.integrations.find(value => value.target === target)?.state === "outdated", `${target} stale hook must be detected`)
    await feature("integration.install", { target })
    const updated = await feature<Integrations>("integration.list")
    check(updated.integrations.find(value => value.target === target)?.state === "current", `${target} update must restore hook`)
    await feature("integration.uninstall", { target })
    const removed = await feature<Integrations>("integration.list")
    check(removed.integrations.find((value) => value.target === target)?.state === "not_installed", `${target} should be removed`)
  }
  check(JSON.parse(await readFile(join(root, "home", ".claude", "settings.json"), "utf8")).fixtureSetting === "preserve", "Claude custom settings must survive uninstall")
  check((await readFile(join(root, "home", ".codex", "config.toml"), "utf8")).includes('model = "e2e-no-provider"'), "Codex custom model config must survive uninstall")
  pass("Claude/Codex integration install, stale detection, update and uninstall preserves custom config (no AI provider)")

  const startedAgent = await feature<{ type: string; agent: { launch_pending: boolean; terminal_id: string } }>("agent.start", { pane_id: paneId, name: "e2e-agent", kind: "pi", args: [], timeout_ms: 8000 })
  transcript.push({ startedAgent })
  check(startedAgent.type === "agent_started" && startedAgent.agent.launch_pending === true && startedAgent.agent.terminal_id === workspace.terminalId, "start acknowledgement must expose the actual pending launch and unchanged terminal")
  const readyDeadline = Date.now() + 10_000
  let agentReady = false
  while (Date.now() < readyDeadline) {
    const { agent: state } = await agentGet(paneId)
    transcript.push({ readiness: state })
    const info = state?.result?.agent
    if (info?.interactive_ready === true && info.launch_pending !== true) { agentReady = true; break }
    await sleep(100)
  }
  check(agentReady, "mock pi must finish launch")
  await feature("agent.wait", { target: "e2e-agent", until: ["idle", "done"], timeout_ms: 8000 })
  const prompted = await feature("agent.prompt", { target: "e2e-agent", text: "E2E prompt with 中文", wait: { until: ["idle", "done"], timeout_ms: 8000 } })
  transcript.push({ prompted })
  check((await readFile(join(root, "agent-prompts.txt"), "utf8")).includes("E2E prompt with 中文"), "mock agent must receive literal Unicode prompt")
  const renamed = await feature<{ agent: { name: string } }>("agent.rename", { target: "e2e-agent", name: "e2e-renamed" })
  check(renamed.agent.name === "e2e-renamed", "agent rename must return live name")
  const explanation = await feature<{ type: string }>("agent.explain", { target: "e2e-renamed" })
  check(explanation.type === "agent_explain", "agent explanation result is required")
  await feature("agent.send_keys", { target: "e2e-renamed", keys: ["ctrl+c"] })
  const stoppedDeadline = Date.now() + 5000
  let agentStopped = false
  while (Date.now() < stoppedDeadline) {
    const { error } = await agentGet(paneId)
    if (error?.includes("agent_not_found")) { agentStopped = true; break }
    await sleep(100)
  }
  check(agentStopped, "Ctrl+C must actually stop the mock agent and release its name")
  const afterKeys = await call<RawSnapshot>("herdr_snapshot", { sessionName: session })
  check(afterKeys.snapshot.panes.some(value => value.pane_id === paneId), "Ctrl+C must preserve its containing pane")
  pass("agent start/prompt/wait/rename/explain/send-keys via local mock pi process (no AI provider)")

  const invalid = await request({ method: "herdrCall", params: { binary, call: { command: "herdr_feature", args: { sessionName: session, request: { method: "agent.wait", params: { target: "absent-agent", until: [], timeout_ms: 0 } } } } } })
  check(invalid.status === "error" && invalid.message.includes("timeout-out-of-range"), "agent wait validation must fail before mutation")
  const explain = await request({ method: "herdrCall", params: { binary, call: { command: "herdr_feature", args: { sessionName: session, request: { method: "agent.explain", params: { target: "absent-agent" } } } } } })
  check(explain.status === "error", "unknown agent must return error")
  pass("agent invalid timeout and unknown target remain errors across production host boundary")

  await feature("session.stop")
  const stopped = await call<HerdrNamedSession[]>("herdr_sessions")
  check(stopped.some((value) => value.name === session && !value.running), "stopped Session must remain saved")
  await feature<{ started: boolean }>("session.start")
  const restored = await call<RawSnapshot>("herdr_snapshot", { sessionName: session })
  check(restored.snapshot.workspaces.some((value) => value.label === "Management E2E"), "restarted Session must restore saved workspace")
  pass("Session stop retains saved state and start/load restores workspaces")
  await feature("session.stop")
  await feature("session.delete")
  const deleted = await call<HerdrNamedSession[]>("herdr_sessions")
  check(!deleted.some((value) => value.name === session), "deleted Session must leave inventory")
  pass("Session stop/delete removes saved inventory")
} catch (error) {
  failure = error instanceof Error ? error.stack : String(error)
  console.error(failure)
  process.exitCode = 1
} finally {
  // All cleanup is addressed only to this fixture Session and isolated env.
  try {
    const status = await cli(["status", "--json"])
    if (status.server?.running) {
      check(contained(status.server.socket), "refuse cleanup outside fixture root")
      await cli(["session", "stop", session, "--json"])
    }
    const finalStatus = await cli(["status", "--json"])
    check(finalStatus.server?.running === false, "fixture server still running after cleanup")
    transcript.push({ cleanup: { session, running: false } })
    pass("cleanup confirms only fixture Session server is stopped")
  } catch (error) { transcript.push({ cleanupError: String(error) }); process.exitCode = 1 }
  host?.stdin.end()
  await sleep(200)
  if (host && host.exitCode === null) host.kill()
  await writeFile(join(root, "evidence.json"), JSON.stringify({ root, session, binary, helper, passes, failure, transcript, hostError }, null, 2))
  console.log(`Evidence: ${join(root, "evidence.json")}`)
}
