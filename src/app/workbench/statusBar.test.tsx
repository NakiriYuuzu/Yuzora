import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";

import { StatusBar } from "@/app/workbench/StatusBar";
import { useContextMenuStore } from "@/state/contextMenuStore";
import { initialGitState, useGitStore } from "@/state/gitStore";
import { useWorkspaceStore } from "@/state/workspaceStore";
import { useLspStore } from "@/state/lspStore";
import { usePreviewStore } from "@/state/previewStore";
import { useUiStore } from "@/state/uiStore";
import { documentGeneration, getDocument } from "@/editor/documentRegistry";
import type { GitStatus, LspServerInfo } from "@/lib/types";

// StatusBar reads the active file's grade through the documentRegistry cache; the
// mock lets each test control that grade without an openFile IPC. documentGeneration
// feeds the grade effect's deps so a same-path reload re-derives the grade.
vi.mock("@/editor/documentRegistry", () => ({
  getDocument: vi.fn(),
  documentGeneration: vi.fn(() => 0),
}));

const initialState = useWorkspaceStore.getState();
const initialUiState = useUiStore.getState();

function makeStatus(): GitStatus {
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
  };
}

function makeServer(over: Partial<LspServerInfo> = {}): LspServerInfo {
  return {
    workspace: "/w",
    language: "python",
    serverId: "Pyright",
    command: "pyright-langserver",
    path: null,
    status: { status: "starting" },
    lastStartupLog: null,
    lastError: null,
    restartCount: 0,
    ...over,
  };
}

function openPython() {
  useWorkspaceStore.setState({
    workspacePath: "/w",
    groups: [
      {
        tabs: [
          { path: "/w/a.py", name: "a.py", dirty: false, externallyModified: false },
        ],
        activePath: "/w/a.py",
      },
    ],
  });
}

describe("StatusBar", () => {
  beforeEach(() => {
    useWorkspaceStore.setState(initialState, true);
    // Merge (not replace) so the store keeps its actions; initialGitState
    // resets every data field the branch segment reads.
    useGitStore.setState(initialGitState);
    useLspStore.getState().reset();
    usePreviewStore.getState().reset();
    // Replace with the captured snapshot so a spied openSettings never leaks.
    useUiStore.setState(initialUiState, true);
    vi.mocked(getDocument).mockResolvedValue({
      result: { kind: "full", content: "", size: 0 },
    });
    vi.mocked(documentGeneration).mockReturnValue(0);
  });

  it("Ready 態顯示 server 名與 Ready", async () => {
    openPython();
    useLspStore.setState({
      servers: { python: makeServer() },
      initialized: { python: true },
    });

    render(<StatusBar />);

    expect(await screen.findByText(/Python · Pyright Ready/)).toBeInTheDocument();
  });

  it("Starting 態顯示 server 名與 Starting 且不可點擊", async () => {
    openPython();
    // status starting + not initialized → starting (spawned, handshake pending).
    useLspStore.setState({
      servers: { python: makeServer() },
      initialized: {},
    });

    render(<StatusBar />);

    expect(await screen.findByText(/Python · Pyright Starting/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Python/ })).not.toBeInTheDocument();
  });

  it("Missing 態可點擊並開啟 LSP 設定對應語言", async () => {
    const openSettings = vi.fn();
    useUiStore.setState({ openSettings });
    openPython();
    useLspStore.setState({
      servers: {
        python: makeServer({
          status: { status: "missing", installHint: "npm i -g pyright" },
        }),
      },
    });

    render(<StatusBar />);

    const btn = await screen.findByRole("button", { name: /Python · Pyright Missing/ });
    expect(btn.getAttribute("title")).toContain("npm i -g pyright");
    fireEvent.click(btn);
    expect(openSettings).toHaveBeenCalledWith("lsp", "python");
  });

  it("Failed 態可點擊且 title 含 stderr 摘要", async () => {
    const openSettings = vi.fn();
    useUiStore.setState({ openSettings });
    openPython();
    useLspStore.setState({
      servers: {
        python: makeServer({
          status: { status: "crashed", reason: "boom" },
          lastError: "spawn pyright ENOENT",
        }),
      },
    });

    render(<StatusBar />);

    const btn = await screen.findByRole("button", { name: /Python · Pyright Failed/ });
    expect(btn.getAttribute("title")).toContain("spawn pyright ENOENT");
    fireEvent.click(btn);
    expect(openSettings).toHaveBeenCalledWith("lsp", "python");
  });

  it("大檔 grade 顯示 Syntax only（無 server 名、不可點擊）", async () => {
    vi.mocked(getDocument).mockResolvedValue({
      result: { kind: "tooLarge", size: 20_000_000 },
    });
    openPython();
    // A live-looking server proves the grade downgrade wins over process state.
    useLspStore.setState({
      servers: { python: makeServer() },
      initialized: { python: true },
    });

    render(<StatusBar />);

    expect(await screen.findByText(/Python · Syntax only/)).toBeInTheDocument();
    expect(screen.queryByText(/Pyright/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Python/ })).not.toBeInTheDocument();
  });

  it("getDocument reject 時退回 Syntax only（不樂觀顯示 LSP 態、不可點擊）", async () => {
    // A stale tab whose file was deleted: the read rejects. The segment must not
    // fall back to an optimistic full grade (which would render a clickable state).
    vi.mocked(getDocument).mockRejectedValue(new Error("gone"));
    openPython();
    useLspStore.setState({
      servers: {
        python: makeServer({
          status: { status: "missing", installHint: "npm i -g pyright" },
        }),
      },
    });

    render(<StatusBar />);

    expect(await screen.findByText(/Python · Syntax only/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Python/ })).not.toBeInTheDocument();
  });

  it("同路徑 reload 後依 documentGeneration 重新推導 grade", async () => {
    openPython();
    useLspStore.setState({
      servers: { python: makeServer() },
      initialized: { python: true },
    });

    render(<StatusBar />);
    expect(await screen.findByText(/Python · Pyright Ready/)).toBeInTheDocument();

    // External reload of the same path: generation bumps and the file now grades
    // tooLarge. The reload flow flips a workspaceStore field, re-rendering the bar
    // so it reads the new generation and re-runs the grade effect.
    vi.mocked(getDocument).mockResolvedValue({
      result: { kind: "tooLarge", size: 20_000_000 },
    });
    vi.mocked(documentGeneration).mockReturnValue(1);
    act(() => {
      useWorkspaceStore.getState().markExternallyModified("/w/a.py", true);
    });

    expect(await screen.findByText(/Python · Syntax only/)).toBeInTheDocument();
  });

  it("非 LSP 語言檔顯示 Lang · Syntax only", async () => {
    useWorkspaceStore.setState({
      workspacePath: "/w",
      groups: [
        {
          tabs: [
            { path: "/w/data.json", name: "data.json", dirty: false, externallyModified: false },
          ],
          activePath: "/w/data.json",
        },
      ],
    });

    render(<StatusBar />);

    expect(await screen.findByText(/JSON · Syntax only/)).toBeInTheDocument();
  });

  it("無開啟檔案時顯示提示", () => {
    render(<StatusBar />);

    expect(screen.getByText(/未開啟檔案/)).toBeInTheDocument();
  });

  it("dev server running 時在中段顯示 port chip", () => {
    useWorkspaceStore.setState({ workspacePath: "/w" });
    usePreviewStore.getState().setDevServer({
      workspace: "/w",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    });

    render(<StatusBar />);

    expect(screen.getByText("Dev 5173")).toBeInTheDocument();
  });

  it("dev server 非 running 時隱藏 port chip", () => {
    useWorkspaceStore.setState({ workspacePath: "/w" });
    usePreviewStore.getState().setDevServer({
      workspace: "/w",
      command: "bun run dev",
      port: 5173,
      status: { status: "exited", code: 0 },
    });

    render(<StatusBar />);

    expect(screen.queryByText(/Dev 5173/)).not.toBeInTheDocument();
  });

  it("右鍵狀態列開啟 status 選單", () => {
    render(<StatusBar />);
    fireEvent.contextMenu(screen.getByLabelText("Status bar"));
    expect(useContextMenuStore.getState().kind).toBe("status");
  });

  it("無 repo 時分支段維持 placeholder 且不可點", () => {
    render(<StatusBar />);
    const branch = screen.getByText("main").closest("button");
    expect(branch).not.toBeNull();
    expect(branch).toBeDisabled();
  });

  it("顯示真實分支名與 ahead 計數", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: { ...makeStatus(), branch: "feature/x", ahead: 2 },
    });

    render(<StatusBar />);

    expect(screen.getByText("feature/x")).toBeInTheDocument();
    expect(screen.getByText("↑2")).toBeInTheDocument();
  });

  it("statusbar shows ahead count and incoming dot in probe mode", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: { ...makeStatus(), branch: "main", ahead: 2 },
      remoteIncoming: "yes",
      remoteCheck: { mode: "probe", intervalSec: 180 },
    });

    render(<StatusBar />);

    expect(screen.getByText("↑2")).toBeInTheDocument();
    expect(screen.getByText("↓•")).toBeInTheDocument();
  });

  it("autofetch 模式下 behind>0 顯示 ↓n", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: { ...makeStatus(), branch: "main", behind: 3 },
      remoteCheck: { mode: "autofetch", intervalSec: 180 },
    });

    render(<StatusBar />);

    expect(screen.getByText("↓3")).toBeInTheDocument();
  });

  it("conflict / in-progress 時分支段加警示色", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: {
        ...makeStatus(),
        branch: "main",
        conflicted: [{ path: "a.ts", origPath: null, status: "UU" }],
        inProgress: "merge",
      },
    });

    render(<StatusBar />);

    const name = screen.getByText("main");
    expect(name).toHaveStyle({ color: "var(--status-d)" });
  });

  it("detached HEAD 顯示 headOid 前 7 碼", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: {
        ...makeStatus(),
        branch: null,
        detached: true,
        headOid: "abcdef1234567890",
      },
    });

    render(<StatusBar />);

    expect(screen.getByText("abcdef1")).toBeInTheDocument();
  });

  it("顯示 changed 計數（unstaged+untracked+conflicted 去重）", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: {
        ...makeStatus(),
        unstaged: [{ path: "a.ts", origPath: null, status: "M" }],
        untracked: ["b.txt"],
        conflicted: [{ path: "c.ts", origPath: null, status: "UU" }],
      },
    });

    render(<StatusBar />);

    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("顯示 conflict 計數（! + 檔數）", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: {
        ...makeStatus(),
        conflicted: [
          { path: "a.ts", origPath: null, status: "UU" },
          { path: "b.ts", origPath: null, status: "UU" },
        ],
      },
    });

    render(<StatusBar />);

    // "!" lives in a bold child span; its parent segment carries the count.
    const bang = screen.getByText("!");
    expect(bang.parentElement).toHaveTextContent("!2");
  });

  it("changed / conflict 皆為 0 時兩段都隱藏", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50" },
      status: makeStatus(),
    });

    render(<StatusBar />);

    expect(screen.queryByText("!")).not.toBeInTheDocument();
  });

  it("非 ready 時不顯示 changed 計數", () => {
    // status 有變更資料但環境未就緒 → 計數段不渲染。
    useGitStore.setState({
      status: {
        ...makeStatus(),
        unstaged: [{ path: "a.ts", origPath: null, status: "M" }],
      },
    });

    render(<StatusBar />);

    expect(screen.queryByText("1")).not.toBeInTheDocument();
  });
});
