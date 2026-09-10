import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SpaceAgentTree } from "./SpaceAgentTree";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import type { HerdrCapabilities, HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes";
import i18n from "@/lib/i18n";

const mocks = vi.hoisted(() => ({ choose: vi.fn(), openTab: vi.fn() }));
vi.mock("@/state/folderPickerStore", async (original) => ({
  ...await original<typeof import("@/state/folderPickerStore")>(),
  chooseWorkspaceFolder: mocks.choose,
}));
vi.mock("@/lib/herdrTabActions", () => ({ openCreatedHerdrTabAndRequestName: mocks.openTab }));
vi.mock("@/lib/platform", async (original) => ({
  ...await original<typeof import("@/lib/platform")>(), isWindowsPlatform: () => true,
}));
vi.mock("./HerdrAgentInspector", () => ({ HerdrAgentInspector: () => null }));

const remote = '["wsl:fixture","default"]';
const originalActions = {
  bootstrap: useHerdrStore.getState().bootstrap,
  refreshSessions: useHerdrStore.getState().refreshSessions,
  selectSession: useHerdrStore.getState().selectSession,
  activateSpace: useHerdrStore.getState().activateSpace,
  createSpaceFromFolder: useHerdrStore.getState().createSpaceFromFolder,
  createTerminalInSelectedSpace: useHerdrStore.getState().createTerminalInSelectedSpace,
};
const createSpace = vi.fn();
const createTerminal = vi.fn();
const activateSpace = vi.fn();
const refreshSessions = vi.fn();
const selectSession = vi.fn();
const capabilities = {
  server: { running: true, compatible: true },
  api: { snapshot: true, workspaceCreate: true, workspaceFocus: true, tabCreate: true },
  terminal: { create: true },
} as HerdrCapabilities;
function runtime(scope: string): HerdrSessionRuntime {
  return {
    connectionState: "ready", errorMessage: null, capabilities, worktreeInventory: null,
    snapshot: { herdrSessionId: scope, spaces: [], agents: [], tabs: [], terminals: [], raw: {} } as unknown as HerdrSnapshot,
  };
}
beforeEach(async () => {
  vi.resetAllMocks();
  await i18n.changeLanguage("zh-TW");
  const localRuntime = runtime("default");
  useHerdrStore.setState({
    ...herdrInitialState, ...localRuntime,
    sessions: [
      { name: "default", running: true, default: true, sessionDir: "C:/fixture", socketPath: "native.sock" },
      { name: "default", runtimeId: remote, hostId: "wsl:fixture", hostLabel: "Ubuntu", running: true, default: true, sessionDir: "/fixture", socketPath: "/fixture/socket" },
    ],
    selectedSessionName: "default",
    runtimesBySession: { default: localRuntime, [remote]: runtime(remote) },
    bootstrap: vi.fn().mockResolvedValue(undefined),
    refreshSessions,
    selectSession,
    activateSpace,
    createSpaceFromFolder: createSpace,
    createTerminalInSelectedSpace: createTerminal,
  });
  createSpace.mockResolvedValue({ ok: true });
  refreshSessions.mockResolvedValue(undefined);
  selectSession.mockImplementation(originalActions.selectSession);
  activateSpace.mockImplementation(async ({ sessionName, workspaceId }) => {
    useHerdrStore.setState({
      ...useHerdrStore.getState().runtimesBySession[sessionName],
      selectedSessionName: sessionName,
      selectedSpaceId: workspaceId,
    });
    return { ok: true };
  });
});
afterEach(() => { cleanup(); useHerdrStore.setState({ ...herdrInitialState, ...originalActions }); });

it.each([
  ["Windows 本機 · default", "default", "local", "C:\\Projects\\native"],
  ["Ubuntu · default", remote, "wsl:fixture", "/home/test/project"],
])("opens a folder and creates the first Space in the empty %s Session", async (label, scope, hostId, path) => {
  mocks.choose.mockResolvedValue(path);
  createSpace.mockImplementation(async () => {
    expect(useHerdrStore.getState().selectedSessionName).toBe(scope);
    return { ok: true };
  });
  render(<SpaceAgentTree />);
  const empty = screen.getByText(`${label}: 此 Session 尚無 Space。`).closest('[role="status"]')!;
  const start = within(empty as HTMLElement).getByRole("button", { name: "開啟資料夾並建立 Space" });
  expect(start).toBeEnabled();
  fireEvent.click(start);
  await waitFor(() => expect(createSpace).toHaveBeenCalledWith(path, path.split(/[\\/]/).at(-1)));
  expect(mocks.choose).toHaveBeenCalledWith({ runtimeHostId: hostId });
  expect(createTerminal).not.toHaveBeenCalled();
});

it("offers one combined folder and Space action without a global terminal action", async () => {
  mocks.choose.mockResolvedValue("C:\\Projects\\native");
  render(<SpaceAgentTree />);
  fireEvent.keyDown(screen.getByRole("button", { name: "新增 Space 或加入 Herdr Session" }), { key: "Enter" });
  const item = await screen.findByRole("menuitem", { name: "開啟資料夾並建立 Space" });
  expect(screen.getAllByRole("menuitem")).toHaveLength(2);
  expect(screen.queryByRole("menuitem", { name: "新增 terminal" })).not.toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "開啟資料夾" })).not.toBeInTheDocument();
  expect(item).not.toHaveAttribute("data-disabled");
  fireEvent.click(item);
  await waitFor(() => expect(createSpace).toHaveBeenCalledOnce());
  expect(createTerminal).not.toHaveBeenCalled();
});

it("keeps the title and current scope in one card, with scope selection separate from add actions", () => {
  render(<SpaceAgentTree />);
  const title = screen.getByText("Spaces 與 Agents");
  const card = title.closest('[data-slot="card"]') as HTMLElement;
  expect(within(card).getByRole("button", { name: "Herdr Session：All" })).toHaveTextContent("全部 Herdr Sessions");
  expect(within(card).getByRole("button", { name: "新增 Space 或加入 Herdr Session" })).toBeEnabled();
  fireEvent.keyDown(within(card).getByRole("button", { name: "Herdr Session：All" }), { key: "Enter" });
  expect(screen.getByRole("menuitemradio", { name: "全部 Herdr Sessions" })).toHaveAttribute("aria-checked", "true");
  expect(screen.queryByRole("menuitem", { name: "開啟資料夾並建立 Space" })).not.toBeInTheDocument();
  expect(screen.queryByRole("menuitem", { name: "加入 Herdr Session" })).not.toBeInTheDocument();
});

async function openSessionPicker() {
  fireEvent.keyDown(screen.getByRole("button", { name: "新增 Space 或加入 Herdr Session" }), { key: "Enter" });
  fireEvent.click(screen.getByRole("menuitem", { name: "加入 Herdr Session" }));
  const dialog = await screen.findByRole("dialog", { name: "加入 Herdr Session" });
  await waitFor(() => expect(within(dialog).getByRole("combobox")).toBeEnabled());
  return dialog;
}

it("offers remote host connections when adding an existing Herdr Session", async () => {
  render(<SpaceAgentTree />);
  const dialog = await openSessionPicker();
  expect(within(dialog).getByRole("tab", { name: "SSH 遠端主機" })).toBeEnabled();
  expect(within(dialog).getByRole("tab", { name: "WSL" })).toBeEnabled();
});

it("refreshes and loads an existing Session from the add menu, then shows its identity in the card", async () => {
  populateBranches();
  render(<SpaceAgentTree />);
  const dialog = await openSessionPicker();
  expect(refreshSessions).toHaveBeenCalledOnce();
  fireEvent.keyDown(within(dialog).getByRole("combobox"), { key: "ArrowDown" });
  fireEvent.click(await screen.findByRole("option", { name: "Ubuntu · default" }));
  fireEvent.click(within(dialog).getByRole("button", { name: "載入 Session" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(selectSession).toHaveBeenCalledWith(remote);
  expect(screen.getByRole("button", { name: "Herdr Session：Ubuntu · default" })).toHaveTextContent("Ubuntu · default");
  expect(screen.getByRole("button", { name: "收合 Ubuntu · default 的 Spaces" })).toBeEnabled();
  expect(screen.queryByRole("button", { name: "收合 Windows 本機 · default 的 Spaces" })).not.toBeInTheDocument();
  expect(screen.queryByText("已載入快照")).not.toBeInTheDocument();
  expect(createSpace).not.toHaveBeenCalled();
  expect(createTerminal).not.toHaveBeenCalled();
  expect(mocks.choose).not.toHaveBeenCalled();
});

it("keeps a failed Session load visible in the picker for recovery", async () => {
  selectSession.mockImplementation(async () => {
    const state = useHerdrStore.getState();
    useHerdrStore.setState({ runtimesBySession: {
      ...state.runtimesBySession,
      default: { ...state.runtimesBySession.default, connectionState: "error", errorMessage: "socket unavailable" },
    } });
  });
  render(<SpaceAgentTree />);
  const dialog = await openSessionPicker();
  fireEvent.click(within(dialog).getByRole("button", { name: "載入 Session" }));
  await waitFor(() => expect(within(dialog).getByRole("alert")).toHaveTextContent("socket unavailable"));
  expect(within(dialog).getByRole("button", { name: "重新整理 Sessions" })).toBeEnabled();
});

it("does not allow a stopped Session to be loaded", async () => {
  useHerdrStore.setState({ sessions: useHerdrStore.getState().sessions.map(session => ({ ...session, running: !session.hostId })) });
  render(<SpaceAgentTree />);
  const dialog = await openSessionPicker();
  fireEvent.keyDown(within(dialog).getByRole("combobox"), { key: "ArrowDown" });
  expect(await screen.findByRole("option", { name: "Ubuntu · default · 尚未執行" })).toHaveAttribute("aria-disabled", "true");
  expect(selectSession).not.toHaveBeenCalled();
});

it("offers refresh instead of loading when no running Sessions exist", async () => {
  useHerdrStore.setState({ sessions: [], selectedSessionName: null });
  render(<SpaceAgentTree />);
  fireEvent.keyDown(screen.getByRole("button", { name: "新增 Space 或加入 Herdr Session" }), { key: "Enter" });
  fireEvent.click(screen.getByRole("menuitem", { name: "加入 Herdr Session" }));
  const dialog = await screen.findByRole("dialog", { name: "加入 Herdr Session" });
  await waitFor(() => expect(within(dialog).getByRole("status")).toHaveTextContent("目前沒有執行中的 Session。"));
  expect(within(dialog).getByRole("button", { name: "載入 Session" })).toBeDisabled();
  expect(within(dialog).getByRole("button", { name: "重新整理 Sessions" })).toBeEnabled();
});

it("does not mutate either Session after cancelling the folder picker", async () => {
  mocks.choose.mockResolvedValue(null);
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getAllByRole("button", { name: "開啟資料夾並建立 Space" })[0]);
  await waitFor(() => expect(mocks.choose).toHaveBeenCalledOnce());
  expect(createSpace).not.toHaveBeenCalled();
  expect(createTerminal).not.toHaveBeenCalled();
});

function populateBranches() {
  const populated = runtime("default");
  populated.snapshot!.spaces = [
    { id: "main", label: "Native project", path: "C:/Projects/native", branch: "main", focused: true, order: 0 },
    { id: "feature", label: "Native feature", path: "C:/Projects/feature", branch: "feature", focused: false, order: 1 },
  ];
  const remoteRuntime = runtime(remote);
  remoteRuntime.snapshot!.spaces = [{ id: "feature", label: "Remote feature", path: "/home/test/feature", branch: "feature", focused: false, order: 0 }];
  useHerdrStore.setState({ ...populated, selectedSpaceId: "main", runtimesBySession: { default: populated, [remote]: remoteRuntime } });
}

it.each([
  ["default", "Windows 本機 · default", "C:/Projects/feature"],
  [remote, "Ubuntu · default", "/home/test/feature"],
])("adds a terminal to the clicked branch in %s, regardless of the current selection", async (scope, label, path) => {
  populateBranches();
  const created = { herdrSessionId: scope, workspaceId: "feature", terminalId: "terminal", title: "Feature", paneId: "pane", tabId: "tab" };
  createTerminal.mockImplementation(async () => {
    expect(useHerdrStore.getState().selectedSessionName).toBe(scope);
    expect(useHerdrStore.getState().selectedSpaceId).toBe("feature");
    return created;
  });
  render(<SpaceAgentTree />);
  const add = screen.getByRole("button", { name: `在 feature 新增 terminal · ${label}` });
  const row = within(add.parentElement!).getByRole("treeitem");
  fireEvent.focus(row);
  expect(add).toHaveAttribute("tabindex", "0");
  fireEvent.click(add);
  await waitFor(() => expect(mocks.openTab).toHaveBeenCalledWith({ sessionName: scope, workspaceId: "feature", terminalId: "terminal", title: "Feature", paneId: "pane", tabId: "tab" }));
  expect(activateSpace).toHaveBeenCalledWith({ sessionName: scope, workspaceId: "feature", path });
  expect(createTerminal).toHaveBeenCalledOnce();
  expect(mocks.choose).not.toHaveBeenCalled();
  expect(createSpace).not.toHaveBeenCalled();
});

it("cancels terminal creation when switching branches is cancelled", async () => {
  populateBranches();
  activateSpace.mockResolvedValue({ ok: false, cancelled: true });
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getByRole("button", { name: "在 feature 新增 terminal · Ubuntu · default" }));
  await waitFor(() => expect(activateSpace).toHaveBeenCalledOnce());
  expect(createTerminal).not.toHaveBeenCalled();
  expect(mocks.openTab).not.toHaveBeenCalled();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("does not create in a different branch if selection changes during activation", async () => {
  populateBranches();
  activateSpace.mockResolvedValue({ ok: true });
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getByRole("button", { name: "在 feature 新增 terminal · Ubuntu · default" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("目前選取的分支已變更"));
  expect(createTerminal).not.toHaveBeenCalled();
});

it("prevents duplicate terminal creation while branch activation is pending and reports failure", async () => {
  populateBranches();
  let resolve!: (result: { ok: false; error: string }) => void;
  activateSpace.mockReturnValue(new Promise(done => { resolve = done; }));
  render(<SpaceAgentTree />);
  const add = screen.getByRole("button", { name: "在 feature 新增 terminal · Ubuntu · default" });
  fireEvent.click(add);
  fireEvent.click(add);
  expect(add).toBeDisabled();
  expect(add).toHaveAttribute("aria-busy", "true");
  expect(activateSpace).toHaveBeenCalledOnce();
  await act(async () => resolve({ ok: false, error: "host disconnected" }));
  expect(screen.getByRole("alert")).toHaveTextContent("host disconnected");
  expect(add).toBeEnabled();
  expect(createTerminal).not.toHaveBeenCalled();
});

it.each(["stale", "incompatible", "unsupported", "missing-capability"])("disables the branch action for a %s target while other hosts stay available", kind => {
  populateBranches();
  const runtimes = useHerdrStore.getState().runtimesBySession;
  const unavailable = { ...runtimes[remote] };
  if (kind === "stale") unavailable.errorMessage = "disconnected";
  else if (kind === "incompatible") unavailable.capabilities = { ...capabilities, server: { running: true, compatible: false } };
  else if (kind === "unsupported") unavailable.connectionState = "unsupported";
  else unavailable.capabilities = { ...capabilities, terminal: { ...capabilities.terminal, create: false } };
  useHerdrStore.setState({ runtimesBySession: { ...runtimes, [remote]: unavailable } });
  render(<SpaceAgentTree />);
  const add = screen.getByRole("button", { name: "在 feature 新增 terminal · Ubuntu · default" });
  expect(add).toBeDisabled();
  expect(screen.getByRole("button", { name: "在 feature 新增 terminal · Windows 本機 · default" })).toBeEnabled();
  fireEvent.click(add);
  expect(activateSpace).not.toHaveBeenCalled();
  expect(createTerminal).not.toHaveBeenCalled();
});

it.each(["stale", "incompatible"])("does not offer creation from a %s cached empty snapshot", kind => {
  const unavailable = runtime("default");
  if (kind === "stale") unavailable.errorMessage = "disconnected";
  else unavailable.capabilities = { ...capabilities, server: { running: true, compatible: false } };
  useHerdrStore.setState({ ...unavailable, sessions: useHerdrStore.getState().sessions.slice(0, 1), runtimesBySession: { default: unavailable } });
  render(<SpaceAgentTree />);
  expect(screen.queryByRole("button", { name: "開啟資料夾並建立 Space" })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("button", { name: "新增 Space 或加入 Herdr Session" }), { key: "Enter" });
  expect(screen.getByRole("menuitem", { name: "開啟資料夾並建立 Space" })).toHaveAttribute("data-disabled");
});

it.each(["connecting", "unsupported", "error"] as const)("keeps %s Sessions gated even with cached capability flags", state => {
  const unavailable = { ...runtime("default"), connectionState: state };
  useHerdrStore.setState({ ...unavailable, sessions: useHerdrStore.getState().sessions.slice(0, 1), runtimesBySession: { default: unavailable } });
  render(<SpaceAgentTree />);
  expect(screen.queryByRole("button", { name: "開啟資料夾並建立 Space" })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("button", { name: "新增 Space 或加入 Herdr Session" }), { key: "Enter" });
  expect(screen.getByRole("menuitem", { name: "開啟資料夾並建立 Space" })).toHaveAttribute("data-disabled");
});

it("does not create on another host if the Session changes while choosing a folder", async () => {
  let resolve!: (path: string) => void;
  mocks.choose.mockReturnValue(new Promise<string>(done => { resolve = done; }));
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getAllByRole("button", { name: "開啟資料夾並建立 Space" })[0]);
  await waitFor(() => expect(mocks.choose).toHaveBeenCalledOnce());
  await act(async () => { await useHerdrStore.getState().selectSession(remote); resolve("C:\\Projects\\native"); });
  expect(createSpace).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Session 已變更");
});
