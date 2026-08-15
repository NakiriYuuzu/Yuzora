import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { StatusBar } from "@/app/workbench/StatusBar";
import { useContextMenuStore } from "@/state/contextMenuStore";
import { initialGitState, useGitStore } from "@/state/gitStore";
import { markdownPreviewPath } from "@/lib/markdownPreviewTab";
import { useWorkspaceStore } from "@/state/workspaceStore";
import { useLspStore } from "@/state/lspStore";
import { usePreviewStore } from "@/state/previewStore";
import { SAMPLING_WINDOW, usePerfStore } from "@/state/perfStore";
import { useUiStore } from "@/state/uiStore";
import { documentGeneration, getDocument } from "@/editor/documentRegistry";
import type { DocumentLineEnding, GitStatus, LspServerInfo } from "@/lib/types";

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

function openPython(lineEnding: DocumentLineEnding = "lf") {
  useWorkspaceStore.setState({
    workspacePath: "/w",
    groups: [
      {
        tabs: [
          {
            path: "/w/a.py",
            name: "a.py",
            dirty: false,
            externallyModified: false,
            lineEnding,
            lineEndingGeneration: 0,
          },
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
    usePerfStore.getState().reset();
    // Replace with the captured snapshot so a spied openSettings never leaks.
    useUiStore.setState(initialUiState, true);
    vi.mocked(getDocument).mockResolvedValue({
      result: { kind: "full", content: "", size: 0, lineEnding: "lf" },
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
      useWorkspaceStore.getState().hydrateLineEnding("/w/a.py", undefined, 1);
      useWorkspaceStore.getState().markExternallyModified("/w/a.py", true);
    });

    expect(await screen.findByText(/Python · Syntax only/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Line ending:/ })).not.toBeInTheDocument();
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

    expect(screen.getByText(/No file open/)).toBeInTheDocument();
  });

  it("editable active file 顯示目前換行格式與 radio selection", async () => {
    openPython("lf");
    render(<StatusBar />);

    const trigger = screen.getByRole("button", { name: "Line ending: LF" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });

    expect(await screen.findByRole("menuitemradio", { name: "Use LF" })).toHaveAttribute(
      "data-state",
      "checked",
    );
    expect(screen.getByRole("menuitemradio", { name: "Use CRLF" })).toHaveAttribute(
      "data-state",
      "unchecked",
    );
  });

  it("active group 切換時 selector 跟著該 group 的 active file", () => {
    useWorkspaceStore.setState({
      workspacePath: "/w",
      activeGroupIndex: 0,
      groups: [
        {
          activePath: "/w/left.ts",
          tabs: [{
            path: "/w/left.ts",
            name: "left.ts",
            dirty: false,
            externallyModified: false,
            lineEnding: "lf",
          }],
        },
        {
          activePath: "/w/right.ts",
          tabs: [{
            path: "/w/right.ts",
            name: "right.ts",
            dirty: false,
            externallyModified: false,
            lineEnding: "crlf",
          }],
        },
      ],
    });
    render(<StatusBar />);
    expect(screen.getByRole("button", { name: "Line ending: LF" })).toBeInTheDocument();

    act(() => useWorkspaceStore.getState().setActiveGroup(1));

    expect(screen.getByRole("button", { name: "Line ending: CRLF" })).toBeInTheDocument();
  });

  it.each([
    ["lf", "Use CRLF", "crlf"],
    ["crlf", "Use LF", "lf"],
    ["mixed", "Use LF", "lf"],
  ] as const)("從 %s 選擇 %s 會更新 metadata 並標記 dirty", async (from, option, target) => {
    openPython(from);
    render(<StatusBar />);
    const label = from === "mixed" ? "Mixed" : from.toUpperCase();
    fireEvent.pointerDown(
      screen.getByRole("button", { name: `Line ending: ${label}` }),
      { button: 0, ctrlKey: false },
    );
    fireEvent.click(await screen.findByRole("menuitemradio", { name: option }));

    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
      lineEnding: target,
      dirty: true,
    });
  });

  it("選擇相同格式為 no-op，不標記 dirty", async () => {
    openPython("lf");
    render(<StatusBar />);
    fireEvent.pointerDown(
      screen.getByRole("button", { name: "Line ending: LF" }),
      { button: 0, ctrlKey: false },
    );
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Use LF" }));

    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
      lineEnding: "lf",
      dirty: false,
    });
  });

  it("Mixed trigger 沒有 radio selection，且可用鍵盤選擇 CRLF 並把 focus 還給 trigger", async () => {
    openPython("mixed");
    render(<StatusBar />);
    const trigger = screen.getByRole("button", { name: "Line ending: Mixed" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    const crlf = await screen.findByRole("menuitemradio", { name: "Use CRLF" });
    expect(screen.getByRole("menuitemradio", { name: "Use LF" })).toHaveAttribute(
      "data-state",
      "unchecked",
    );
    expect(crlf).toHaveAttribute("data-state", "unchecked");

    crlf.focus();
    fireEvent.keyDown(crlf, { key: "Enter" });

    await waitFor(() =>
      expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
        lineEnding: "crlf",
        dirty: true,
      }),
    );
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("preview 與沒有 editable metadata 的 readonly/unknown tab 隱藏 selector", () => {
    useWorkspaceStore.setState({
      workspacePath: "/w",
      activeGroupIndex: 0,
      groups: [{
        activePath: "/w/legacy.txt",
        tabs: [{
          path: "/w/legacy.txt",
          name: "legacy.txt",
          dirty: false,
          externallyModified: false,
        }],
      }],
    });
    const { rerender } = render(<StatusBar />);
    expect(screen.queryByRole("button", { name: /Line ending:/ })).not.toBeInTheDocument();

    act(() => useWorkspaceStore.getState().openPreviewTab());
    rerender(<StatusBar />);
    expect(screen.queryByRole("button", { name: /Line ending:/ })).not.toBeInTheDocument();
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
    expect(useContextMenuStore.getState().request?.kind).toBe("status");
  });

  it("有 perf snapshot 時顯示總量 chip（cpu% · MB），title 拆出 App 本體與子行程數", () => {
    usePerfStore.getState().setSnapshot({
      cpuPercent: 12,
      memoryBytes: 370_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 102_000_000,
      descendantCount: 3,
      webviewCpuPercent: 0,
      webviewMemoryBytes: 0,
      webviewCount: 0,
      managedToolsCpuPercent: 0,
      managedToolsMemoryBytes: 0,
      managedToolsCount: 0,
    });

    render(<StatusBar />);

    // 主要數字是 app + descendants 的總量，不是 app 本體（#22）。
    const chip = screen.getByText("12% · 370MB");
    expect(chip).toBeInTheDocument();
    const title = chip.getAttribute("title");
    // scope label：tooltip 必須說明這是「App + 子行程」的總量。issue #22 的預期
    // 結果第二句要求 UI 標示統計範圍，退回「主程序」語意就是回歸。
    expect(title).toContain("App + managed child processes");
    expect(title).not.toContain("Main process");
    expect(title).toContain("App 4% · 102MB");
    expect(title).toContain("3 child processes");
  });

  it("沒有子行程時 title 顯示 0 個且總量等於 App 本體", () => {
    usePerfStore.getState().setSnapshot({
      cpuPercent: 4,
      memoryBytes: 102_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 102_000_000,
      descendantCount: 0,
      webviewCpuPercent: 0,
      webviewMemoryBytes: 0,
      webviewCount: 0,
      managedToolsCpuPercent: 0,
      managedToolsMemoryBytes: 0,
      managedToolsCount: 0,
    });

    render(<StatusBar />);

    const chip = screen.getByText("4% · 102MB");
    expect(chip.getAttribute("title")).toContain("0 child processes");
  });

  // --- issue #40 §3.3 / §3.5 ---------------------------------------------

  it("title 拆出 WebView renderer/GPU 與受管理工具的分類小計", () => {
    usePerfStore.getState().setSnapshot({
      cpuPercent: 12,
      memoryBytes: 592_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 50_000_000,
      descendantCount: 7,
      webviewCpuPercent: 6,
      webviewMemoryBytes: 542_000_000,
      webviewCount: 6,
      managedToolsCpuPercent: 2,
      managedToolsMemoryBytes: 0,
      managedToolsCount: 1,
    });

    render(<StatusBar />);

    const title = screen.getByTestId("status-perf-chip").getAttribute("title");
    // 原症狀（issue #40）：6 個 WebView descendants 合計約 542 MB，而 host 只有 49 MB。
    expect(title).toContain("6 WebView renderer/GPU 542MB");
    expect(title).toContain("1 managed tools 0MB");
  });

  it("採樣失敗時 chip 帶警示標記，且仍顯示最新的數值", () => {
    usePerfStore.getState().setSnapshot({
      cpuPercent: 12,
      memoryBytes: 370_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 102_000_000,
      descendantCount: 3,
      webviewCpuPercent: 0,
      webviewMemoryBytes: 0,
      webviewCount: 0,
      managedToolsCpuPercent: 8,
      managedToolsMemoryBytes: 268_000_000,
      managedToolsCount: 3,
    });
    usePerfStore.getState().recordOutcome("failed", "perf_snapshot boom");

    render(<StatusBar />);

    const chip = screen.getByTestId("status-perf-chip");
    // 優先序：失敗只是**附加**標記，不取代每次 poll 都會更新的數值。
    expect(chip.textContent).toBe("12% · 370MB ⚠");
    const title = chip.getAttribute("title");
    expect(title).toContain("App + managed child processes");
    expect(title).toContain("1 failed, 0 returned no data out of the last 1 polls");
    expect(title).toContain("perf_snapshot boom");
  });

  it("還沒有快照但採樣一直失敗時，chip 仍要出現（否則失敗又變回看不見）", () => {
    usePerfStore.getState().recordOutcome("failed", "perf_snapshot boom");

    render(<StatusBar />);

    const chip = screen.getByTestId("status-perf-chip");
    expect(chip.textContent).toContain("perf n/a");
    expect(chip.getAttribute("title")).toContain("Perf sampling:");
  });

  it("採樣恢復後警示標記消失", () => {
    usePerfStore.getState().recordOutcome("failed", "boom");
    usePerfStore.getState().setSnapshot({
      cpuPercent: 12,
      memoryBytes: 370_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 102_000_000,
      descendantCount: 3,
      webviewCpuPercent: 0,
      webviewMemoryBytes: 0,
      webviewCount: 0,
      managedToolsCpuPercent: 8,
      managedToolsMemoryBytes: 268_000_000,
      managedToolsCount: 3,
    });
    for (let index = 0; index < SAMPLING_WINDOW; index += 1) {
      usePerfStore.getState().recordOutcome("ok");
    }

    render(<StatusBar />);

    const chip = screen.getByTestId("status-perf-chip");
    expect(chip.textContent).toBe("12% · 370MB");
    expect(chip.getAttribute("title")).not.toContain("Perf sampling:");
  });

  it("只是失焦跳過（沒有失敗）不會被當成錯誤顯示", () => {
    usePerfStore.getState().recordOutcome("skipped_no_focus");

    render(<StatusBar />);

    expect(screen.queryByTestId("status-perf-chip")).not.toBeInTheDocument();
  });

  // `Ok(None)`（後端有回應但沒有資料）與 reject 一樣是「這次沒量到」，只是走另
  // 一條路徑。Major 3 的整個要點就是它不可以靜默，而**使用者實際看到的就是這個
  // chip**——所以防護必須釘在這一層，不能只釘在 store 與落盤層。
  it("empty（Ok(None)）也要讓 chip 帶警示，且不抹掉最後一次成功的快照", () => {
    usePerfStore.getState().setSnapshot({
      cpuPercent: 12,
      memoryBytes: 370_000_000,
      appCpuPercent: 4,
      appMemoryBytes: 102_000_000,
      descendantCount: 3,
      webviewCpuPercent: 0,
      webviewMemoryBytes: 0,
      webviewCount: 0,
      managedToolsCpuPercent: 8,
      managedToolsMemoryBytes: 268_000_000,
      managedToolsCount: 3,
    });
    usePerfStore.getState().recordOutcome("empty");

    render(<StatusBar />);

    const chip = screen.getByTestId("status-perf-chip");
    // 一次 reject 都沒有，但仍必須有 ⚠——否則「後端回 null」就靜默了。
    expect(chip.textContent).toBe("12% · 370MB ⚠");
    expect(chip.getAttribute("data-perf-sampling-failures")).toBe("0");
    expect(chip.getAttribute("data-perf-sampling-empty")).toBe("1");
    const title = chip.getAttribute("title");
    expect(title).toContain("0 failed, 1 returned no data out of the last 1 polls");
  });

  it("從未成功且一直回 empty 時，chip 顯示 perf n/a ⚠ 而不是整個消失", () => {
    for (let index = 0; index < 5; index += 1) {
      usePerfStore.getState().recordOutcome("empty");
    }

    render(<StatusBar />);

    const chip = screen.getByTestId("status-perf-chip");
    expect(chip.textContent).toContain("perf n/a");
    expect(chip.textContent).toContain("⚠");
    expect(chip.getAttribute("data-perf-sampling-empty")).toBe("5");
    expect(chip.getAttribute("title")).toContain(
      "0 failed, 5 returned no data out of the last 5 polls",
    );
  });

  it("perf snapshot 為 null 時不顯示 chip", () => {
    render(<StatusBar />);

    expect(screen.queryByText(/MB/)).not.toBeInTheDocument();
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

  it("active markdown preview tab does not read the synthetic path as a file", () => {
    const previewPath = markdownPreviewPath("/w/readme.md");
    useWorkspaceStore.setState({
      workspacePath: "/w",
      groups: [{
        activePath: previewPath,
        tabs: [{
          path: previewPath,
          name: "Preview",
          dirty: false,
          externallyModified: false,
          kind: "markdown-preview",
          sourcePath: "/w/readme.md",
        }],
      }],
      activeGroupIndex: 0,
    });
    vi.mocked(getDocument).mockClear();
    render(<StatusBar />);
    expect(getDocument).not.toHaveBeenCalled();
  });
});
