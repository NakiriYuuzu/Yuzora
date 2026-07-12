import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import {
  CONTEXT_MENU_DEFS,
  commandFor,
  resolveContextMenuEntries,
} from "@/app/workbench/contextMenuDefs"
import type { ContextMenuKind, ContextMenuRequest } from "@/app/workbench/contextMenuModel"
import i18n from "@/lib/i18n"
import { registerView, unregisterView, updateViewMetadata } from "@/editor/viewRegistry"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useAgentStore, type SessionState } from "@/state/agentStore"
import { terminalInitialState, useTerminalStore } from "@/state/terminalStore"
import { registerTerminalView } from "@/terminal/terminalViewRegistry"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { useGitRollbackDialogStore } from "@/state/gitRollbackDialogStore"
import { useDbStore } from "@/state/dbStore"
import { useSftpStore } from "@/state/sftpStore"
import { useSshStore } from "@/state/sshStore"
import { usePreviewStore } from "@/state/previewStore"
import { gitChangeRows } from "@/workbench/git/gitChangeSelection"
import type { GitStatus } from "@/lib/types"

const FINAL_KINDS: ContextMenuKind[] = [
  "general",
  "rail",
  "explorer",
  "file",
  "tab",
  "editor",
  "terminal",
  "agentSession",
  "git",
  "gitChange",
  "status",
  "sshhost",
  "dbconn",
  "preview",
]

function agentSession(overrides: Partial<SessionState> = {}): SessionState {
  return {
    title: "Clicked session",
    agentLabel: "Agent",
    model: null,
    tone: "idle",
    transcript: [],
    availableCommands: [],
    stopReason: null,
    stopBadge: null,
    error: null,
    queueDepth: null,
    running: null,
    pendingTurn: false,
    metadataTitle: false,
    cwd: "/w/project",
    ...overrides,
  }
}

function gitStatus(overrides: Partial<GitStatus> = {}): GitStatus {
  return {
    branch: "main",
    headOid: "0".repeat(40),
    detached: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    staged: [],
    unstaged: [],
    untracked: [],
    conflicted: [],
    inProgress: null,
    ...overrides,
  }
}

function gitChangeRequest(status: GitStatus): ContextMenuRequest {
  const rows = gitChangeRows(status)
  if (!rows[0]) throw new Error("status must contain a Git change")
  return {
    kind: "gitChange",
    repositoryRoot: "/w",
    clicked: { ...rows[0] },
    selected: rows.map((row) => ({ ...row })),
  }
}

beforeEach(async () => {
  clearMocks()
  mockIPC((cmd) => cmd === "log_event" ? null : undefined)
  unregisterView("/w/edit.ts")
  await i18n.changeLanguage("en")
  useWorkspaceStore.setState({
    workspacePath: "/w",
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
  })
  useTerminalStore.setState(terminalInitialState)
  useAgentStore.setState({
    sessions: new Map(),
    activeSessionId: null,
    renamingSessionId: null,
    confirmRemoveRequest: null,
  })
  useGitStore.setState(initialGitState)
  useDbStore.setState({
    connections: [],
    activeConnId: null,
    saved: [],
    sessions: {},
    tables: {},
    queries: {},
    reconnectRequest: null,
    reconnectRequestToken: 0,
  })
  useSshStore.setState({
    hosts: [],
    sessions: {},
    activeHostId: null,
    pendingAuthHostId: null,
  })
  useSftpStore.getState().reset()
  usePreviewStore.getState().reset()
  if (useGitRollbackDialogStore.getState().pending) {
    useGitRollbackDialogStore.getState().respond(false)
  }
})

describe("CONTEXT_MENU_DEFS", () => {
  it("涵蓋 final kind set", () => {
    expect(Object.keys(CONTEXT_MENU_DEFS).sort()).toEqual([...FINAL_KINDS].sort())
  })

  it("每個非 separator entry 都具備完整 command contract", () => {
    for (const entries of Object.values(CONTEXT_MENU_DEFS)) {
      for (const entry of entries) {
        if (entry === "separator") continue
        expect(entry.id).not.toBe("")
        expect(entry.label).toBeTypeOf("function")
        expect(entry.availability).toBeTypeOf("function")
        expect(entry.danger).toBeTypeOf("boolean")
        expect(entry.executor).toBeTypeOf("function")
      }
    }
  })

  it("label resolver 依目前語言動態解析", async () => {
    const request: ContextMenuRequest = { kind: "general" }
    const command = commandFor(request, "cmSettings")
    expect(command?.label(request)).toBe("Settings…")
    await i18n.changeLanguage("zh-TW")
    expect(command?.label(request)).toBe("設定…")
  })

  it("File/Tab split labels match their distinct semantics in both locales", async () => {
    const fileRequest: ContextMenuRequest = {
      kind: "file",
      workspacePath: "/w",
      path: "/w/a.ts",
      isDirectory: false,
      sourceGroupIndex: 0,
    }
    const tabRequest: ContextMenuRequest = {
      kind: "tab",
      workspacePath: "/w",
      path: "/w/a.ts",
      groupIndex: 0,
    }
    expect(commandFor(fileRequest, "cmOpenSplit")?.label(fileRequest)).toBe("Open in Right Split")
    expect(commandFor(tabRequest, "cmSplit")?.label(tabRequest)).toBe("Split and Move Right")
    await i18n.changeLanguage("zh-TW")
    expect(commandFor(fileRequest, "cmOpenSplit")?.label(fileRequest)).toBe("在右側分割開啟")
    expect(commandFor(tabRequest, "cmSplit")?.label(tabRequest)).toBe("分割並移至右側")
  })

  it("rail 使用 Open Workspace，picker cancel 回傳 cancelled", async () => {
    const request: ContextMenuRequest = { kind: "rail" }
    expect(commandFor(request, "cmNewProject")).toBeNull()
    const command = commandFor(request, "cmOpenWorkspace")
    expect(command?.label(request)).toBe("Open workspace…")
    mockIPC((cmd) => cmd === "plugin:dialog|open" ? null : undefined)
    expect(await command?.executor(request)).toBe("cancelled")
  })

  it("hidden entry 後正規化 separator，沒有 leading/trailing/double separator", () => {
    const entries = resolveContextMenuEntries({ kind: "general" })
    expect(entries.map((entry) => entry.type === "separator" ? "|" : entry.command.id)).toEqual([
      "cmCmdPalette",
      "|",
      "cmSettings",
      "cmHideSidebar",
    ])
    expect(entries[0]?.type).toBe("command")
    expect(entries.at(-1)?.type).toBe("command")
    expect(entries.some((entry, index) => entry.type === "separator" && entries[index - 1]?.type === "separator")).toBe(false)
  })

  it("HTML-only action 以 typed file target 決定 visible", () => {
    const html = resolveContextMenuEntries({
      kind: "file",
      workspacePath: "/w",
      path: "/w/index.html",
      isDirectory: false,
      sourceGroupIndex: 0,
    })
    const source = resolveContextMenuEntries({
      kind: "file",
      workspacePath: "/w",
      path: "/w/main.ts",
      isDirectory: false,
      sourceGroupIndex: 0,
    })
    expect(html.some((entry) => entry.type === "command" && entry.command.id === "cmOpenInBrowser")).toBe(true)
    expect(source.some((entry) => entry.type === "command" && entry.command.id === "cmOpenInBrowser")).toBe(false)
  })

  it("directory 隱藏 file-only open actions，但保留 entity actions", () => {
    const entries = resolveContextMenuEntries({
      kind: "file",
      workspacePath: "/w",
      path: "/w/src",
      isDirectory: true,
      sourceGroupIndex: 0,
    })
    const ids = entries.flatMap((entry) => entry.type === "command" ? [entry.command.id] : [])
    expect(ids).not.toContain("cmOpen")
    expect(ids).not.toContain("cmOpenSplit")
    expect(ids).not.toContain("cmOpenInBrowser")
    expect(ids).toEqual(expect.arrayContaining(["cmRename", "cmCopyRel", "cmReveal", "cmDelete"]))
  })

  it("Explorer Refresh only exists for the request workspace and bumps treeRevision", async () => {
    expect(resolveContextMenuEntries({ kind: "explorer", workspacePath: null })).toEqual([])
    const request: ContextMenuRequest = { kind: "explorer", workspacePath: "/w" }
    const command = commandFor(request, "cmRefresh")
    const before = useWorkspaceStore.getState().treeRevision
    expect(await command?.executor(request)).toBe("completed")
    expect(useWorkspaceStore.getState().treeRevision).toBe(before + 1)
  })

  it("general 完全移除 Refresh，Explorer Refresh 在 workspace 存在時 enabled", () => {
    expect(commandFor({ kind: "general" }, "cmRefresh")).toBeNull()
    const entries = resolveContextMenuEntries({ kind: "explorer", workspacePath: "/w" })
    const refresh = entries.find((entry) => entry.type === "command" && entry.command.id === "cmRefresh")
    expect(refresh?.type).toBe("command")
    if (refresh?.type === "command") expect(refresh.availability).toEqual({ visible: true, enabled: true })
  })

  it("最右 group 達兩組上限時 split disabled 且帶 i18n reason", () => {
    useWorkspaceStore.setState({
      groups: [
        { tabs: [], activePath: null },
        {
          tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false }],
          activePath: "/w/a.ts",
        },
      ],
      activeGroupIndex: 1,
    })
    const entries = resolveContextMenuEntries({
      kind: "tab",
      workspacePath: "/w",
      path: "/w/a.ts",
      groupIndex: 1,
    })
    const split = entries.find((entry) => entry.type === "command" && entry.command.id === "cmSplit")
    expect(split?.type).toBe("command")
    if (split?.type === "command") {
      expect(split.availability).toEqual({
        visible: true,
        enabled: false,
        disabledReasonKey: "contextMenu.disabled.twoGroupLimit",
      })
    }
  })

  it("Preview sentinel tab 隱藏 Split and Move Right", () => {
    useWorkspaceStore.getState().openPreviewTab()
    const entries = resolveContextMenuEntries({
      kind: "tab",
      workspacePath: "/w",
      path: "yuzora://preview",
      groupIndex: 0,
    })
    expect(entries.some(
      (entry) => entry.type === "command" && entry.command.id === "cmSplit"
    )).toBe(false)
  })

  it("editor availability follows the clicked view selection, readonly and formatter metadata", () => {
    const path = "/w/edit.ts"
    const view = new EditorView({
      state: EditorState.create({ doc: "hello", selection: { anchor: 0, head: 5 } })
    })
    useWorkspaceStore.setState({
      groups: [{
        tabs: [{ path, name: "edit.ts", dirty: false, externallyModified: false }],
        activePath: path,
      }],
      activeGroupIndex: 0,
    })
    registerView(path, view, {
      groupIndex: 0,
      readonly: false,
      formatter: "checking",
    })
    const request: ContextMenuRequest = {
      kind: "editor",
      workspacePath: "/w",
      path,
      groupIndex: 0,
    }
    const byId = () => new Map(resolveContextMenuEntries(request).flatMap(
      (entry) => entry.type === "command" ? [[entry.command.id, entry]] : []
    ))

    expect(byId().get("cmCopy")?.availability.enabled).toBe(true)
    expect(byId().get("cmCut")?.availability.enabled).toBe(true)
    expect(byId().get("cmPaste")?.availability.enabled).toBe(true)
    expect(byId().get("cmFormatDoc")?.availability).toMatchObject({
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.formatterChecking",
    })

    view.dispatch({ selection: { anchor: 0 } })
    expect(byId().get("cmCopy")?.availability.disabledReasonKey).toBe("contextMenu.disabled.noSelection")
    expect(byId().get("cmCut")?.availability.disabledReasonKey).toBe("contextMenu.disabled.noSelection")
    view.dispatch({ selection: { anchor: 0, head: 5 } })

    updateViewMetadata(path, view, {
      readonly: true,
      formatter: "available",
      formatDocument: async () => true,
    })
    expect(byId().get("cmCopy")?.availability.enabled).toBe(true)
    expect(byId().get("cmCut")?.availability.disabledReasonKey).toBe("contextMenu.disabled.readonly")
    expect(byId().get("cmPaste")?.availability.disabledReasonKey).toBe("contextMenu.disabled.readonly")
    expect(byId().get("cmFormatDoc")?.availability.disabledReasonKey).toBe("contextMenu.disabled.readonly")

    updateViewMetadata(path, view, {
      readonly: false,
      formatter: "unsupported",
      formatDocument: undefined,
    })
    expect(byId().has("cmFormatDoc")).toBe(false)
    unregisterView(path, view)
    view.destroy()
  })

  it("terminal availability follows the clicked pane, xterm selection and pane cap", async () => {
    const request: ContextMenuRequest = {
      kind: "terminal",
      workspacePath: "/w",
      paneId: "pane-clicked",
      sessionId: "session-clicked",
    }
    useTerminalStore.setState({
      sessions: {
        "session-clicked": {
          sessionId: "session-clicked",
          title: "Terminal 1",
          workspace: "/w",
          shell: "",
          cols: 80,
          rows: 24,
        },
      },
      layouts: {
        "/w": {
          panes: [{ paneId: "pane-clicked", sessionId: "session-clicked" }],
          activePaneId: "unrelated-active-pane",
          splitDirection: null,
        },
      },
    })
    let selected = false
    const unregister = registerTerminalView("session-clicked", {
      hasSelection: () => selected,
      getSelection: () => "output",
      paste: () => undefined,
      clear: () => undefined,
    })
    const byId = () => new Map(resolveContextMenuEntries(request).flatMap(
      (entry) => entry.type === "command" ? [[entry.command.id, entry]] : []
    ))

    expect(byId().get("cmCopySel")?.availability).toMatchObject({
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noSelection",
    })
    expect(byId().get("cmPaste")?.availability.enabled).toBe(true)
    expect(byId().get("cmClear")?.availability.enabled).toBe(true)
    expect(byId().get("cmCloseTerminal")?.availability.enabled).toBe(true)
    expect(byId().get("cmSplitTermRight")?.availability.enabled).toBe(true)
    expect(byId().has("cmDockTerm")).toBe(false)
    expect(byId().has("cmKill")).toBe(false)

    selected = true
    expect(byId().get("cmCopySel")?.availability.enabled).toBe(true)

    useTerminalStore.getState().splitFrom(
      "/w",
      "pane-clicked",
      {
        sessionId: "session-2",
        title: "Terminal 2",
        workspace: "/w",
        shell: "",
        cols: 80,
        rows: 24,
      },
      "right"
    )
    expect(byId().get("cmSplitTermRight")?.availability).toMatchObject({
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.terminalPaneLimit",
    })
    expect(commandFor(request, "cmClear")?.label(request)).toBe("Clear buffer")
    await i18n.changeLanguage("zh-TW")
    expect(commandFor(request, "cmCloseTerminal")?.label(request)).toBe("關閉終端機")
    unregister()
  })

  it("agentSession registry 只保留 final actions，並提供精確文案與 pending disabled reason", async () => {
    const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
    useAgentStore.setState({
      activeSessionId: "other",
      sessions: new Map([
        ["clicked", agentSession()],
        ["other", agentSession({ pendingTurn: true })],
      ]),
    })

    const entries = resolveContextMenuEntries(request)
    expect(entries.map((entry) => entry.type === "separator" ? "|" : entry.command.id)).toEqual([
      "cmContinueSession",
      "cmCancelResponse",
      "cmRenameSession",
      "|",
      "cmCopyWorkingDirectory",
      "|",
      "cmRemoveSession",
    ])
    expect(commandFor(request, "cmCancelResponse")?.availability(request)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noPendingResponse",
    })
    expect(commandFor(request, "cmRenameSession")?.label(request)).toBe("Rename display name…")
    expect(commandFor(request, "cmStop")).toBeNull()
    expect(commandFor(request, "cmCopyPath")).toBeNull()
    expect(commandFor(request, "cmDuplicate")).toBeNull()

    await i18n.changeLanguage("zh-TW")
    expect(commandFor(request, "cmCancelResponse")?.label(request)).toBe("取消目前回應")
    expect(commandFor(request, "cmCopyWorkingDirectory")?.label(request)).toBe("複製工作目錄路徑")
  })

  it("agentSession availability matrix：session 不存在／存在時的 continue、rename、remove；cancelResponse 依 pendingTurn", () => {
    const missing: ContextMenuRequest = { kind: "agentSession", sessionId: "ghost" }
    const idle: ContextMenuRequest = { kind: "agentSession", sessionId: "idle-session" }
    const running: ContextMenuRequest = { kind: "agentSession", sessionId: "running-session" }
    useAgentStore.setState({
      sessions: new Map([
        ["idle-session", agentSession()],
        ["running-session", agentSession({ pendingTurn: true })],
      ]),
    })

    for (const id of ["cmContinueSession", "cmRenameSession", "cmRemoveSession"] as const) {
      expect(commandFor(missing, id)?.availability(missing)).toEqual({
        visible: true,
        enabled: false,
        disabledReasonKey: "contextMenu.disabled.targetUnavailable",
      })
      expect(commandFor(idle, id)?.availability(idle)).toEqual({ visible: true, enabled: true })
    }
    expect(commandFor(missing, "cmCancelResponse")?.availability(missing)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.targetUnavailable",
    })
    expect(commandFor(idle, "cmCancelResponse")?.availability(idle)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noPendingResponse",
    })
    expect(commandFor(running, "cmCancelResponse")?.availability(running)).toEqual({
      visible: true,
      enabled: true,
    })
  })

  it("agentSession executors 只作用 request.sessionId：continue／rename 觸發對應 store action", async () => {
    const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
    useAgentStore.setState({
      activeSessionId: "other",
      sessions: new Map([
        ["clicked", agentSession()],
        ["other", agentSession()],
      ]),
    })

    const continueSession = commandFor(request, "cmContinueSession")
    if (!continueSession) throw new Error("missing cmContinueSession")
    expect(await continueSession.executor(request)).toBe("completed")
    expect(useAgentStore.getState().activeSessionId).toBe("clicked")

    const rename = commandFor(request, "cmRenameSession")
    if (!rename) throw new Error("missing cmRenameSession")
    expect(await rename.executor(request)).toBe("completed")
    expect(useAgentStore.getState().renamingSessionId).toBe("clicked")
  })

  it("agentSession cmRemoveSession executor 等待 confirm：拒絕時 cancelled 且不移除，確認時 completed 並移除 clicked session", async () => {
    const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
    useAgentStore.setState({
      sessions: new Map([
        ["clicked", agentSession()],
        ["other", agentSession()],
      ]),
    })
    const remove = commandFor(request, "cmRemoveSession")
    if (!remove) throw new Error("missing cmRemoveSession")

    const declined = remove.executor(request)
    useAgentStore.getState().respondRemoveSessionConfirm(false)
    expect(await declined).toBe("cancelled")
    expect(useAgentStore.getState().sessions.has("clicked")).toBe(true)

    const confirmed = remove.executor(request)
    useAgentStore.getState().respondRemoveSessionConfirm(true)
    expect(await confirmed).toBe("completed")
    expect(useAgentStore.getState().sessions.has("clicked")).toBe(false)
    expect(useAgentStore.getState().sessions.has("other")).toBe(true)

    useAgentStore.setState({
      sessions: new Map([
        ["clicked", agentSession()],
        ["other", agentSession()],
      ]),
    })
    const stale = remove.executor(request)
    useAgentStore.setState({ sessions: new Map([["other", agentSession()]]) })
    useAgentStore.getState().respondRemoveSessionConfirm(true)
    expect(await stale).toBe("cancelled")
    expect(useAgentStore.getState().sessions.has("other")).toBe(true)
  })

  it("agentSession Copy Working Directory 只顯示並複製 clicked session 的 absolute cwd", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args })
      return cmd === "log_event" ? null : undefined
    })
    const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
    useAgentStore.setState({
      activeSessionId: "other",
      sessions: new Map([
        ["clicked", agentSession({ cwd: "/clicked/project" })],
        ["other", agentSession({ cwd: "/active/project" })],
      ]),
    })
    const copy = commandFor(request, "cmCopyWorkingDirectory")
    if (!copy) throw new Error("missing cmCopyWorkingDirectory")

    expect(await copy.executor(request)).toBe("completed")
    expect(calls.find((call) => call.cmd === "plugin:clipboard-manager|write_text")?.args)
      .toEqual({ text: "/clicked/project" })

    useAgentStore.setState({
      sessions: new Map([["clicked", agentSession({ cwd: "relative/project" })]]),
    })
    expect(resolveContextMenuEntries(request).some(
      (entry) => entry.type === "command" && entry.command.id === "cmCopyWorkingDirectory"
    )).toBe(false)
  })

  it("dbconn Open/Reconnect label and disabled reason follow live state in both locales", async () => {
    const request: ContextMenuRequest = {
      kind: "dbconn",
      descriptorId: "db-saved",
      address: "u@h:5432/d",
    }
    const sqlite = {
      id: "db-saved",
      targetKey: "/a.db",
      kind: "sqlite" as const,
      name: "a.db",
      path: "/a.db",
    }
    const network = {
      id: "db-saved",
      targetKey: "postgres:h:5432:d",
      kind: "postgres" as const,
      name: "d@h",
      host: "h",
      port: 5432,
      database: "d",
      user: "u",
      ssl: false,
      credentialState: "stored" as const,
    }
    const command = commandFor(request, "cmOpenDb")
    if (!command) throw new Error("missing cmOpenDb")
    const copyCommand = commandFor(request, "cmCopyAddr")
    if (!copyCommand) throw new Error("missing cmCopyAddr")

    useDbStore.setState({ saved: [sqlite], connections: [], sessions: {}, activeConnId: null })
    expect(command.label(request)).toBe("Reconnect")
    expect(copyCommand.label(request)).toBe("Copy database file path")
    expect(command.availability(request)).toEqual({ visible: true, enabled: true })

    useDbStore.setState({ saved: [network] })
    expect(command.label(request)).toBe("Reconnect")
    expect(copyCommand.label(request)).toBe("Copy database address")
    useDbStore.setState({ saved: [{ ...network, credentialState: "required" as const }] })
    expect(command.label(request)).toBe("Reconnect…")
    useDbStore.setState({ saved: [{ ...network, credentialState: "unavailable" as const }] })
    expect(command.label(request)).toBe("Reconnect…")
    useDbStore.setState({ saved: [network] })

    useDbStore.setState({
      sessions: {
        "db-saved": {
          descriptorId: "db-saved",
          connId: null,
          status: "connecting",
          error: null,
        },
      },
    })
    const connecting = command.availability(request)
    expect(connecting).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.dbConnecting",
    })
    expect(i18n.t(connecting.disabledReasonKey!, { ns: "menus" })).toBe("Connecting")

    useDbStore.setState({
      saved: [sqlite],
      sessions: {},
      connections: [{
        connId: "live-1",
        kind: "sqlite",
        name: "a.db",
        descriptorId: "db-saved",
        targetKey: "/a.db",
        title: "/a.db",
      }],
      activeConnId: null,
    })
    expect(command.label(request)).toBe("Open")
    expect(command.availability(request)).toEqual({ visible: true, enabled: true })

    useDbStore.setState({ activeConnId: "live-1" })
    const active = command.availability(request)
    expect(command.label(request)).toBe("Open")
    expect(active).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.dbAlreadyOpen",
    })
    expect(i18n.t(active.disabledReasonKey!, { ns: "menus" })).toBe("Already open")

    await i18n.changeLanguage("zh-TW")
    expect(command.label(request)).toBe("開啟")
    expect(copyCommand.label(request)).toBe("複製資料庫檔案路徑")
    expect(i18n.t(active.disabledReasonKey!, { ns: "menus" })).toBe("目前已開啟")
    useDbStore.setState({
      connections: [],
      activeConnId: null,
      saved: [{ ...network, credentialState: "required" }],
      sessions: {},
    })
    expect(command.label(request)).toBe("重新連線…")
    expect(copyCommand.label(request)).toBe("複製資料庫位址")
    useDbStore.setState({ saved: [network] })
    expect(command.label(request)).toBe("重新連線")
    useDbStore.setState({ saved: [sqlite] })
    expect(command.label(request)).toBe("重新連線")
    expect(copyCommand.label(request)).toBe("複製資料庫檔案路徑")
    useDbStore.setState({
      sessions: {
        "db-saved": {
          descriptorId: "db-saved",
          connId: null,
          status: "connecting",
          error: null,
        },
      },
    })
    expect(i18n.t(command.availability(request).disabledReasonKey!, { ns: "menus" })).toBe("正在連線")
  })

  it("dbconn menu executor uses the shared command for focus, reconnect requests and stale rechecks", async () => {
    mockIPC((cmd) => {
      if (cmd === "db_profile_open") {
        throw { code: "credentialRequired", message: "RAW CREDENTIAL DETAIL" }
      }
      return cmd === "log_event" ? null : undefined
    })
    const request: ContextMenuRequest = {
      kind: "dbconn",
      descriptorId: "pg-saved",
      address: "u@h:5432/d",
    }
    const network = {
      id: "pg-saved",
      targetKey: "postgres:h:5432:d",
      kind: "postgres" as const,
      name: "d@h",
      host: "h",
      port: 5432,
      database: "d",
      user: "u",
      ssl: false,
    }
    const command = commandFor(request, "cmOpenDb")
    if (!command) throw new Error("missing cmOpenDb")

    useDbStore.setState({ saved: [network], reconnectRequest: null })
    expect(await command.executor(request)).toBe("completed")
    expect(useDbStore.getState().reconnectRequest).toEqual({ descriptorId: "pg-saved", token: 1 })
    expect(await command.executor(request)).toBe("completed")
    expect(useDbStore.getState().reconnectRequest).toEqual({ descriptorId: "pg-saved", token: 2 })

    useDbStore.setState({
      connections: [{
        connId: "pg-live",
        kind: "postgres",
        name: "d@h",
        descriptorId: "pg-saved",
        targetKey: network.targetKey,
        title: "u@h:5432/d",
      }],
      activeConnId: null,
      sessions: {},
      tables: { "pg-live": [] },
    })
    expect(await command.executor(request)).toBe("completed")
    expect(useDbStore.getState().activeConnId).toBe("pg-live")
    expect(await command.executor(request)).toBe("cancelled")

    useDbStore.setState({ saved: [] })
    expect(await command.executor(request)).toBe("cancelled")
    expect(useDbStore.getState().activeConnId).toBe("pg-live")
  })

  it("dbconn Copy Address re-resolves the clicked descriptor instead of a stale address snapshot", async () => {
    const clipboardCalls: Array<{ cmd: string; args: unknown }> = []
    mockIPC((cmd, args) => {
      clipboardCalls.push({ cmd, args })
      return undefined
    })
    const request: ContextMenuRequest = {
      kind: "dbconn",
      descriptorId: "pg-saved",
      address: "stale@old.example:1/old",
    }
    useDbStore.setState({
      saved: [{
        id: "pg-saved",
        targetKey: "postgres:current.example:5432:app",
        kind: "postgres",
        name: "app",
        host: "current.example",
        port: 5432,
        database: "app",
        user: "admin",
        ssl: false,
      }],
    })

    expect(await commandFor(request, "cmCopyAddr")?.executor(request)).toBe("completed")
    expect(clipboardCalls.find((call) => call.cmd === "plugin:clipboard-manager|write_text")?.args)
      .toMatchObject({ text: "admin@current.example:5432/app" })
  })

  it("dbconn Disconnect is visible only for a live connection", () => {
    const request: ContextMenuRequest = {
      kind: "dbconn",
      descriptorId: "db-saved",
      address: "/a.db",
    }
    useDbStore.setState({
      saved: [{ id: "db-saved", targetKey: "/a.db", kind: "sqlite", name: "a.db", path: "/a.db" }],
      connections: [],
    })
    expect(commandFor(request, "cmDisconnect")?.availability(request)).toEqual({
      visible: false,
      enabled: false,
    })
    expect(resolveContextMenuEntries(request).some(
      (entry) => entry.type === "command" && entry.command.id === "cmDisconnect"
    )).toBe(false)

    useDbStore.setState({
      connections: [{
        connId: "db-live",
        kind: "sqlite",
        name: "a.db",
        descriptorId: "db-saved",
        targetKey: "/a.db",
        title: "/a.db",
      }],
    })
    expect(commandFor(request, "cmDisconnect")?.availability(request)).toEqual({
      visible: true,
      enabled: true,
    })
  })

  it("sshhost availability and executors use only the clicked host, never activeHostId", async () => {
    const clipboardCalls: Array<{ cmd: string; args: unknown }> = []
    mockIPC((cmd, args) => {
      clipboardCalls.push({ cmd, args })
      return cmd === "log_event" ? null : undefined
    })
    const clicked = {
      id: "clicked",
      name: "Clicked",
      host: "clicked.example.com",
      port: 2222,
      user: "clicked-user",
      authKind: "password" as const,
    }
    const other = {
      id: "other",
      name: "Other",
      host: "other.example.com",
      port: 22,
      user: "other-user",
      authKind: "password" as const,
    }
    const request: ContextMenuRequest = {
      kind: "sshhost",
      hostId: clicked.id,
      address: "stale@address:1",
    }
    useSshStore.setState({
      hosts: [clicked, other],
      sessions: {
        other: {
          hostId: other.id,
          sessionId: "other-session",
          status: "connected",
          fingerprint: null,
          knownHost: true,
          error: null,
        },
      },
      activeHostId: other.id,
      pendingAuthHostId: null,
    })

    expect(resolveContextMenuEntries(request).map((entry) =>
      entry.type === "separator" ? "|" : entry.command.id
    )).toEqual(["cmOpenSsh", "cmOpenSftp", "|", "cmCopyAddr"])

    expect(await commandFor(request, "cmOpenSsh")?.executor(request)).toBe("completed")
    expect(useSshStore.getState().pendingAuthHostId).toBe(clicked.id)
    expect(useSshStore.getState().sessions[other.id]?.sessionId).toBe("other-session")
    expect(await commandFor(request, "cmOpenSsh")?.executor(request)).toBe("cancelled")

    useSshStore.getState().cancelPendingAuth()
    expect(await commandFor(request, "cmOpenSftp")?.executor(request)).toBe("completed")
    expect(useSftpStore.getState().activeTab).toBe("sftp")
    expect(useSshStore.getState().activeHostId).toBe(clicked.id)
    expect(useSshStore.getState().pendingAuthHostId).toBe(clicked.id)

    useSshStore.setState({ pendingAuthHostId: null })
    expect(await commandFor(request, "cmCopyAddr")?.executor(request)).toBe("completed")
    expect(clipboardCalls.find((call) => call.cmd === "plugin:clipboard-manager|write_text")?.args)
      .toEqual({ text: "clicked-user@clicked.example.com:2222" })

    useSshStore.setState({ pendingAuthHostId: clicked.id })
    const pending = commandFor(request, "cmOpenSsh")?.availability(request)
    expect(pending).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.authenticationPending",
    })
    expect(i18n.t(pending?.disabledReasonKey ?? "", { ns: "menus" }))
      .toBe("Authentication is awaiting input")

    useSshStore.setState({
      pendingAuthHostId: null,
      sessions: {
        ...useSshStore.getState().sessions,
        clicked: {
          hostId: clicked.id,
          sessionId: null,
          status: "connecting",
          fingerprint: null,
          knownHost: false,
          error: null,
        },
      },
    })
    expect(commandFor(request, "cmOpenSftp")?.availability(request)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.connecting",
    })
    expect(await commandFor(request, "cmOpenSftp")?.executor(request)).toBe("cancelled")
    expect(commandFor(request, "cmDisconnect")?.availability(request)).toEqual({
      visible: false,
      enabled: false,
    })

    useSshStore.setState({
      sessions: {
        ...useSshStore.getState().sessions,
        clicked: {
          hostId: clicked.id,
          sessionId: "clicked-session",
          status: "connected",
          fingerprint: null,
          knownHost: true,
          error: null,
        },
      },
    })
    expect(commandFor(request, "cmDisconnect")?.availability(request)).toEqual({
      visible: true,
      enabled: true,
    })

    const missing: ContextMenuRequest = { kind: "sshhost", hostId: "missing", address: "missing" }
    expect(resolveContextMenuEntries(missing)).toEqual([])

    await i18n.changeLanguage("zh-TW")
    useSshStore.setState({ pendingAuthHostId: clicked.id })
    const pendingZh = commandFor(request, "cmOpenSsh")?.availability(request)
    expect(i18n.t(pendingZh?.disabledReasonKey ?? "", { ns: "menus" }))
      .toBe("正在等待驗證資料")
  })

  it("git/status copy presence and remote-operation availability follow the requested repository", () => {
    const gitRequest: ContextMenuRequest = { kind: "git", repositoryRoot: "/w" }
    const statusRequest: ContextMenuRequest = { kind: "status", repositoryRoot: "/w" }
    const operationIds = ["cmFetch", "cmPull", "cmPush"]
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status: gitStatus({ branch: "feature/menu", headOid: "a".repeat(40) }),
      busy: null,
    })

    for (const request of [gitRequest, statusRequest]) {
      expect(resolveContextMenuEntries(request).map((entry) =>
        entry.type === "separator" ? "|" : entry.command.id
      )).toEqual(["cmCopyHash", "cmCopyBranch", "|", ...operationIds])
    }

    useGitStore.setState({ status: gitStatus({ branch: null, headOid: "b".repeat(40) }) })
    expect(commandFor(statusRequest, "cmCopyHash")?.availability(statusRequest).visible).toBe(true)
    expect(commandFor(statusRequest, "cmCopyBranch")?.availability(statusRequest).visible).toBe(false)

    useGitStore.setState({ status: gitStatus({ branch: "main", headOid: undefined }) })
    expect(commandFor(statusRequest, "cmCopyHash")?.availability(statusRequest).visible).toBe(false)
    expect(commandFor(statusRequest, "cmCopyBranch")?.availability(statusRequest).visible).toBe(true)

    useGitStore.setState({ status: gitStatus({ branch: null, headOid: undefined }) })
    expect(resolveContextMenuEntries(statusRequest).map((entry) =>
      entry.type === "separator" ? "|" : entry.command.id
    )).toEqual(operationIds)

    useGitStore.setState({ environment: { status: "notARepo" }, busy: null })
    for (const id of operationIds) {
      expect(commandFor(statusRequest, id)?.availability(statusRequest)).toEqual({
        visible: true,
        enabled: false,
        disabledReasonKey: "contextMenu.disabled.notRepository",
      })
    }
    expect(commandFor(statusRequest, "cmCopyHash")?.availability(statusRequest).visible).toBe(false)
    expect(commandFor(statusRequest, "cmCopyBranch")?.availability(statusRequest).visible).toBe(false)

    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status: gitStatus(),
      busy: "pull",
    })
    for (const id of operationIds) {
      expect(commandFor(gitRequest, id)?.availability(gitRequest)).toEqual({
        visible: true,
        enabled: false,
        disabledReasonKey: "contextMenu.disabled.gitBusy",
      })
    }
  })

  it("git root 完全移除 Stage/Rollback placeholders；bulk actions 只留在 toolbar", () => {
    const request: ContextMenuRequest = { kind: "git", repositoryRoot: "/w" }
    expect(commandFor(request, "cmStage")).toBeNull()
    expect(commandFor(request, "cmRollback")).toBeNull()
    expect(CONTEXT_MENU_DEFS.git.some((entry) =>
      entry !== "separator" && (entry.id === "cmStage" || entry.id === "cmRollback")
    )).toBe(false)
  })

  it("gitChange mixed selection shows Stage/Unstage subsets and Rollback", () => {
    const status = gitStatus({
      staged: [{ path: "staged.ts", origPath: null, status: "M" }],
      unstaged: [{ path: "changed.ts", origPath: null, status: "M" }],
      conflicted: [{ path: "conflict.ts", origPath: null, status: "UU" }],
    })
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status,
    })
    const request = gitChangeRequest(status)
    expect(resolveContextMenuEntries(request).map((entry) =>
      entry.type === "separator" ? "|" : entry.command.id
    )).toEqual([
      "cmStageSelected",
      "cmUnstageSelected",
      "|",
      "cmRollbackSelected",
    ])
  })

  it("gitChange mixed Stage/Unstage executors operate only their applicable subsets", async () => {
    const status = gitStatus({
      staged: [{ path: "staged.ts", origPath: null, status: "M" }],
      unstaged: [{ path: "changed.ts", origPath: null, status: "M" }],
      conflicted: [{ path: "conflict.ts", origPath: null, status: "UU" }],
    })
    const calls: Array<{ cmd: string; args: unknown }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args })
      return null
    })
    const runOp = vi.fn(async (_name: string, operation: () => Promise<unknown>) => {
      await operation()
      return true
    })
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status,
      runOp,
    })
    const request = gitChangeRequest(status)

    expect(await commandFor(request, "cmStageSelected")?.executor(request)).toBe("completed")
    expect(calls.find((call) => call.cmd === "git_stage")?.args).toEqual({
      repositoryRoot: "/w",
      paths: ["changed.ts", "conflict.ts"],
    })
    expect(runOp).toHaveBeenLastCalledWith("stage", expect.any(Function))

    expect(await commandFor(request, "cmUnstageSelected")?.executor(request)).toBe("completed")
    expect(calls.find((call) => call.cmd === "git_unstage")?.args).toEqual({
      repositoryRoot: "/w",
      paths: ["staged.ts"],
    })
    expect(runOp).toHaveBeenLastCalledWith("unstage", expect.any(Function))
  })

  it("gitChange busy keeps applicable actions visible and reason names current operation", () => {
    const status = gitStatus({ untracked: ["new.ts"] })
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status,
      busy: "pull",
    })
    const request = gitChangeRequest(status)
    const stage = commandFor(request, "cmStageSelected")?.availability(request)
    expect(stage).toMatchObject({ visible: true, enabled: false })
    expect(stage?.disabledReasonKey).toContain("pull")
  })

  it("gitChange exact snapshot drift M→R/origPath hides stale commands", () => {
    const original = gitStatus({
      unstaged: [{ path: "renamed.ts", origPath: null, status: "M" }],
    })
    const request = gitChangeRequest(original)
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status: gitStatus({
        unstaged: [{ path: "renamed.ts", origPath: "old.ts", status: "R" }],
      }),
    })
    expect(resolveContextMenuEntries(request)).toEqual([])
    expect(commandFor(request, "cmStageSelected")?.availability(request)).toEqual({
      visible: false,
      enabled: false,
    })
    expect(commandFor(request, "cmRollbackSelected")?.availability(request)).toEqual({
      visible: false,
      enabled: false,
    })
  })

  it("gitChange Rollback executor awaits the app-level dialog outcome", async () => {
    const status = gitStatus({ untracked: ["scratch.ts"] })
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status,
    })
    const request = gitChangeRequest(status)
    const rollback = commandFor(request, "cmRollbackSelected")
    if (!rollback) throw new Error("missing cmRollbackSelected")

    const cancelled = rollback.executor(request)
    expect(useGitRollbackDialogStore.getState().pending).toMatchObject({
      repositoryRoot: "/w",
      targets: [{ path: "scratch.ts", classification: "untracked" }],
    })
    useGitRollbackDialogStore.getState().respond(false)
    await expect(cancelled).resolves.toBe("cancelled")

    const completed = rollback.executor(request)
    useGitRollbackDialogStore.getState().respond(true)
    await expect(completed).resolves.toBe("completed")
  })

  it("preview availability follows URL history, stable workspace and running attempt", async () => {
    const emptyRequest: ContextMenuRequest = {
      kind: "preview",
      workspacePath: "/w",
      url: null,
      serverAttempt: 0,
    }
    const empty = resolveContextMenuEntries(emptyRequest)
    expect(empty.map((entry) => entry.type === "separator" ? "|" : entry.command.id)).toEqual([
      "cmPreviewBack",
      "cmPreviewForward",
    ])
    expect(commandFor(emptyRequest, "cmPreviewBack")?.availability(emptyRequest)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noBackHistory",
    })
    expect(commandFor(emptyRequest, "cmPreviewForward")?.availability(emptyRequest)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noForwardHistory",
    })

    const preview = usePreviewStore.getState()
    preview.navigate("/w", "http://localhost:5173")
    preview.navigate("/w", "http://localhost:5173/about")
    const request: ContextMenuRequest = {
      kind: "preview",
      workspacePath: "/w",
      url: "http://localhost:5173/about",
      serverAttempt: preview.attemptForWorkspace("/w"),
    }
    expect(commandFor(request, "cmPreviewBack")?.availability(request).enabled).toBe(true)
    expect(commandFor(request, "cmPreviewForward")?.availability(request)).toMatchObject({
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.noForwardHistory",
    })
    expect(commandFor(request, "cmPreviewReload")?.availability(request).enabled).toBe(true)
    expect(await commandFor(request, "cmPreviewReload")?.executor(request)).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/w").reloadNonce).toBe(1)

    preview.setDevServer({
      workspace: "/w",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })
    expect(commandFor(request, "cmStopDevServer")?.availability(request)).toEqual({
      visible: true,
      enabled: true,
    })
    preview.beginAttempt("/w")
    expect(commandFor(request, "cmStopDevServer")?.availability(request)).toEqual({
      visible: false,
      enabled: false,
    })

    preview.navigate("/w", "http://localhost:5173/contact")
    expect(commandFor(request, "cmPreviewReload")?.availability(request)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.targetUnavailable",
    })
    useWorkspaceStore.setState({ workspacePath: "/other" })
    expect(commandFor(request, "cmPreviewBack")?.availability(request)).toEqual({
      visible: true,
      enabled: false,
      disabledReasonKey: "contextMenu.disabled.targetUnavailable",
    })
  })

  it("representative final-kind matrix has executable enabled entries, reasons for disabled entries, and normalized separators", () => {
    const status = gitStatus({ untracked: ["new.ts"] })
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status,
      busy: null,
    })
    useSshStore.setState({
      hosts: [{
        id: "host-1",
        name: "Host",
        host: "example.com",
        port: 22,
        user: "user",
        authKind: "password",
      }],
      sessions: {},
      activeHostId: null,
      pendingAuthHostId: null,
    })
    const change = gitChangeRequest(status)
    const requests: ContextMenuRequest[] = [
      { kind: "general" },
      { kind: "rail" },
      { kind: "explorer", workspacePath: "/w" },
      { kind: "file", workspacePath: "/w", path: "/w/a.ts", isDirectory: false, sourceGroupIndex: 0 },
      { kind: "tab", workspacePath: "/w", path: "/w/a.ts", groupIndex: 0 },
      { kind: "editor", workspacePath: "/w", path: "/w/a.ts", groupIndex: 0 },
      { kind: "terminal", workspacePath: "/w", paneId: "pane", sessionId: "session" },
      { kind: "agentSession", sessionId: "missing-session" },
      { kind: "git", repositoryRoot: "/w" },
      change,
      { kind: "status", repositoryRoot: "/w" },
      { kind: "sshhost", hostId: "host-1", address: "user@example.com:22" },
      { kind: "dbconn", descriptorId: "missing-db", address: "missing" },
      { kind: "preview", workspacePath: "/w", url: null, serverAttempt: 0 },
    ]

    expect(requests.map((request) => request.kind)).toEqual(FINAL_KINDS)
    for (const request of requests) {
      const entries = resolveContextMenuEntries(request)
      expect(entries.length, request.kind).toBeGreaterThan(0)
      expect(entries[0]?.type, request.kind).toBe("command")
      expect(entries.at(-1)?.type, request.kind).toBe("command")
      for (const [index, entry] of entries.entries()) {
        if (entry.type === "separator") {
          expect(entries[index - 1]?.type, request.kind).toBe("command")
          expect(entries[index + 1]?.type, request.kind).toBe("command")
          continue
        }
        if (entry.availability.enabled) {
          expect(entry.command.executor, `${request.kind}:${entry.command.id}`).toBeTypeOf("function")
        } else {
          expect(entry.availability.disabledReasonKey, `${request.kind}:${entry.command.id}`).toBeTruthy()
        }
      }
    }
  })
})
