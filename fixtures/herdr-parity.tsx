/** Production dialogs and xterm; all external mutations are replaced by fixture IPC. */
import { createRoot } from "react-dom/client"
import type { Channel } from "@tauri-apps/api/core"
import { installDemoRuntime, capabilities, sessions, snapshot } from "../src/demo/runtime"
import { normalizeHerdrSnapshot } from "../src/lib/herdrNormalize"
import { useHerdrStore } from "../src/state/herdrStore"
import { useHerdrToolsStore, type HerdrTool } from "../src/state/herdrToolsStore"
import { useHerdrNativeStore } from "../src/state/herdrNativeStore"
import { HerdrToolsHost } from "../src/workbench/HerdrToolsHost"
import { Button } from "../src/components/ui/button"
import { AppDialogHost } from "../src/workbench/AppDialogHost"
import type { HerdrTerminalEvent } from "../src/lib/herdrTypes"
import type { HerdrFeatureRequest } from "../src/lib/herdrFeatures"
import "../src/styles.css"
import i18n from "../src/lib/i18n"

installDemoRuntime()
const normalized = normalizeHerdrSnapshot(snapshot, "studio")
const methods = ["worktree.list", "worktree.create", "worktree.open", "worktree.remove", "pane.move", "agent.start", "agent.prompt", "agent.wait", "agent.rename", "agent.send_keys", "agent.explain", "integration.list", "integration.install", "integration.uninstall", "plugin.list", "plugin.enable", "plugin.disable", "plugin.action.invoke", "plugin.pane.open", "plugin.log.list"]
const caps = { ...capabilities, server: { ...capabilities.server, running: true, compatible: true }, api: { ...capabilities.api, methods: [...capabilities.api.methods, ...methods] } }
useHerdrStore.setState({ sessions, selectedSessionName: "studio", capabilities: caps, snapshot: normalized, runtimesBySession: { studio: { capabilities: caps, snapshot: normalized, connectionState: "ready", errorMessage: null, worktreeInventory: null } } })
const calls: { command: string; args: Record<string, unknown> }[] = []
const fixtureSessions = [...sessions, { ...sessions[0], name: "parked", default: false, running: false }]
useHerdrStore.setState({ sessions: fixtureSessions })
let failMethod: string | null = null
let failRefresh = false
let operationDelay = 0
let pluginEnabled = true
let clipboardText = ""
let permissionGranted = false
// Tauri replaces this browser API in its webview; keep fixture prompts local too.
Notification.requestPermission = async () => permissionGranted ? "granted" : "denied"
const scenario = new URLSearchParams(location.search).get("scenario")
if (scenario === "fail-create") failMethod = "worktree.create"
if (scenario === "fail-refresh") failRefresh = true
if (scenario === "slow-create") operationDelay = 1500
if (scenario === "dark") document.documentElement.classList.add("dark")
let channel: Channel<HerdrTerminalEvent> | null = null
let seq = 0
function frame(text: string) {
  const bytes = new TextEncoder().encode(text)
  channel?.onmessage({ type: "frame", sessionId: "fixture-client", seq: ++seq, full: false, encoding: "ansi", width: 120, height: 40, bytesBase64: btoa(String.fromCharCode(...bytes)) })
}
function graphics(z = 0) {
  const rgba = new Uint8Array(32 * 32 * 4)
  for (let i = 0; i < 32 * 32; i++) rgba.set([i % 32 < 16 ? 235 : 30, 80, Math.floor(i / 32) < 16 ? 65 : 225, 255], i * 4)
  const encoded = btoa(String.fromCharCode(...rgba))
  frame("\x1b[2J\x1b[HOfficial-client stream fixture\r\n搜尋 / Copy mode / Plugin popup\x1b[5;3HText under image")
  frame(`\x1b[5;3H\x1b_Ga=T,f=32,s=32,v=32,i=42,p=1,c=18,r=7,z=${z},C=1,q=2,m=1;${encoded.slice(0, 4096)}\x1b\\`)
  frame(`\x1b_Gm=0;${encoded.slice(4096)}\x1b\\`)
}
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__
const original = internals.invoke
internals.invoke = async (command, args) => {
  calls.push({ command, args })
  console.info("HERDR fixture IPC", JSON.stringify({ command, args: { ...args, onEvent: undefined } }))
  if (command === "herdr_capabilities") {
    if (failRefresh) { failRefresh = false; throw new Error("Fixture refresh failure") }
    return caps
  }
  if (command === "herdr_sessions") return [...fixtureSessions]
  if (command === "herdr_worktree_list") return { repositories: [], worktrees: [
    { path: "/fixture/linked", label: "Linked checkout", branch: "fixture/linked", isLinkedWorktree: true, openWorkspaceId: "linked" },
    { path: "/fixture/closed", label: "Closed checkout", branch: "fixture/closed", isLinkedWorktree: true, openWorkspaceId: null }
  ] }
  if (command === "plugin:clipboard-manager|write_text") { clipboardText = String(args.text); return null }
  if (command === "plugin:notification|is_permission_granted") return permissionGranted
  if (command === "plugin:notification|request_permission") return permissionGranted ? "granted" : "denied"
  if (command === "herdr_client_open") {
    channel = args.onEvent as Channel<HerdrTerminalEvent>
    setTimeout(() => graphics(scenario === "negative-graphics" ? -1 : 0), 100)
    return { sessionId: "fixture-client", target: "studio", mode: "control", role: "controller", cols: 120, rows: 40, takeover: false }
  }
  if (["herdr_terminal_input", "herdr_terminal_resize", "herdr_terminal_release", "herdr_pane_focus"].includes(command)) return null
  if (command === "herdr_feature") {
    const request = args.request as HerdrFeatureRequest
    if (request.method === failMethod) { failMethod = null; throw new Error("Fixture operation failure") }
    if (operationDelay && !request.method.endsWith(".list")) await new Promise(resolve => setTimeout(resolve, operationDelay))
    if (request.method === "integration.list") return { type: "integration_list", integrations: [{ target: "codex", label: "Codex", command: "codex", available: true, state: "current" }, { target: "claude", label: "Claude Code", command: "claude", available: true, state: "outdated" }] }
    if (request.method === "plugin.list") return { type: "plugin_list", plugins: [{ plugin_id: "fixture.demo", name: "Fixture plugin", version: "1.0.0", enabled: pluginEnabled, actions: [{ id: "inspect", title: "Inspect" }], panes: [{ id: "popup", title: "Open popup", placement: "popup" }] }] }
    if (request.method === "plugin.disable") pluginEnabled = false
    if (request.method === "plugin.enable") pluginEnabled = true
    if (request.method.startsWith("session.")) {
      const name = String(args.sessionName)
      const session = fixtureSessions.find(item => item.name === name)
      if (request.method === "session.start") {
        if (session) session.running = true
        else fixtureSessions.push({ ...sessions[0], name, default: false })
      }
      if (request.method === "session.stop" && session) session.running = false
      if (request.method === "session.delete" && session) fixtureSessions.splice(fixtureSessions.indexOf(session), 1)
    }
    if (request.method === "plugin.pane.open" || request.method === "plugin.action.invoke") frame("\x1b[16;3HPLUGIN POPUP — controlled fixture, no host process")
    return { type: "fixture_result", messages: ["Fixture only: request captured; no host mutation executed."] }
  }
  return original(command, args)
}
const tools: HerdrTool[] = ["worktrees", "agents", "panes", "sessions", "integrations", "plugins", "notifications"]
createRoot(document.getElementById("root")!).render(<main className="p-6"><h1>HERDR acceptance fixture · controlled IPC</h1><div className="flex flex-wrap gap-2 py-4">{tools.map(tool => <Button key={tool} onClick={() => useHerdrToolsStore.getState().open({ tool, sessionName: "studio" })}>{tool}</Button>)}<Button onClick={() => useHerdrNativeStore.getState().open({ sessionName: "studio" })}>Full Session</Button></div><HerdrToolsHost /><AppDialogHost /></main>)
Object.assign(window, { herdrParity: {
  calls, graphics, frame,
  failNext: (method: string) => { failMethod = method },
  failNextRefresh: () => { failRefresh = true },
  delay: (milliseconds: number) => { operationDelay = milliseconds },
  permission: (granted: boolean) => { permissionGranted = granted },
  clipboard: () => clipboardText,
  language: (language: string) => i18n.changeLanguage(language)
} })
