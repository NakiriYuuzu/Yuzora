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
  createSpaceFromFolder: useHerdrStore.getState().createSpaceFromFolder,
  createTerminalInSelectedSpace: useHerdrStore.getState().createTerminalInSelectedSpace,
};
const createSpace = vi.fn();
const createTerminal = vi.fn();
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
    createSpaceFromFolder: createSpace,
    createTerminalInSelectedSpace: createTerminal,
  });
  createSpace.mockResolvedValue({ ok: true });
});
afterEach(() => { cleanup(); useHerdrStore.setState({ ...herdrInitialState, ...originalActions }); });

it.each([
  ["Windows 本機 · default", "default", "local", "C:\\Projects\\native"],
  ["Ubuntu · default", remote, "wsl:fixture", "/home/test/project"],
])("offers a first-terminal action on the empty %s Session", async (label, scope, hostId, path) => {
  mocks.choose.mockResolvedValue(path);
  createSpace.mockImplementation(async () => {
    expect(useHerdrStore.getState().selectedSessionName).toBe(scope);
    return { ok: true };
  });
  render(<SpaceAgentTree />);
  const empty = screen.getByText(`${label}: 此 Session 尚無 Space。`).closest('[role="status"]')!;
  const start = within(empty as HTMLElement).getByRole("button", { name: "選擇資料夾並開啟 terminal" });
  expect(start).toBeEnabled();
  fireEvent.click(start);
  await waitFor(() => expect(createSpace).toHaveBeenCalledWith(path, path.split(/[\\/]/).at(-1)));
  expect(mocks.choose).toHaveBeenCalledWith({ runtimeHostId: hostId });
  expect(createTerminal).not.toHaveBeenCalled();
});

it("routes New terminal through folder selection when the selected Session has no Space", async () => {
  mocks.choose.mockResolvedValue("C:\\Projects\\native");
  render(<SpaceAgentTree />);
  fireEvent.keyDown(screen.getByRole("button", { name: "Herdr Session：All" }), { key: "Enter" });
  const item = await screen.findByRole("menuitem", { name: "新增 terminal" });
  expect(item).not.toHaveAttribute("data-disabled");
  fireEvent.click(item);
  await waitFor(() => expect(createSpace).toHaveBeenCalledOnce());
  expect(createTerminal).not.toHaveBeenCalled();
});

it("does not mutate either Session after cancelling the folder picker", async () => {
  mocks.choose.mockResolvedValue(null);
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getAllByRole("button", { name: "選擇資料夾並開啟 terminal" })[0]);
  await waitFor(() => expect(mocks.choose).toHaveBeenCalledOnce());
  expect(createSpace).not.toHaveBeenCalled();
  expect(createTerminal).not.toHaveBeenCalled();
});

it("creates another terminal in the selected Space without opening a folder picker", async () => {
  const populated = runtime("default");
  populated.snapshot!.spaces = [{ id: "space", label: "Native project", path: "C:/Projects/native", focused: true, order: 0 }];
  useHerdrStore.setState({ ...populated, selectedSpaceId: "space", runtimesBySession: { default: populated } });
  const created = { herdrSessionId: "default", workspaceId: "space", terminalId: "terminal", title: "Native project", paneId: "pane", tabId: "tab" };
  createTerminal.mockResolvedValue(created);
  render(<SpaceAgentTree />);
  fireEvent.keyDown(screen.getByRole("button", { name: "Herdr Session：All" }), { key: "Enter" });
  fireEvent.click(screen.getByRole("menuitem", { name: "新增 terminal" }));
  await waitFor(() => expect(mocks.openTab).toHaveBeenCalledWith({ sessionName: "default", workspaceId: "space", terminalId: "terminal", title: "Native project", paneId: "pane", tabId: "tab" }));
  expect(mocks.choose).not.toHaveBeenCalled();
  expect(createSpace).not.toHaveBeenCalled();
});

it.each(["stale", "incompatible"])("does not offer creation from a %s cached empty snapshot", kind => {
  const unavailable = runtime("default");
  if (kind === "stale") unavailable.errorMessage = "disconnected";
  else unavailable.capabilities = { ...capabilities, server: { running: true, compatible: false } };
  useHerdrStore.setState({ ...unavailable, sessions: useHerdrStore.getState().sessions.slice(0, 1), runtimesBySession: { default: unavailable } });
  render(<SpaceAgentTree />);
  expect(screen.queryByRole("button", { name: "選擇資料夾並開啟 terminal" })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("button", { name: "Herdr Session：All" }), { key: "Enter" });
  expect(screen.getByRole("menuitem", { name: "新增 terminal" })).toHaveAttribute("data-disabled");
});

it.each(["connecting", "unsupported", "error"] as const)("keeps %s Sessions gated even with cached capability flags", state => {
  const unavailable = { ...runtime("default"), connectionState: state };
  useHerdrStore.setState({ ...unavailable, sessions: useHerdrStore.getState().sessions.slice(0, 1), runtimesBySession: { default: unavailable } });
  render(<SpaceAgentTree />);
  expect(screen.queryByRole("button", { name: "選擇資料夾並開啟 terminal" })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("button", { name: "Herdr Session：All" }), { key: "Enter" });
  expect(screen.getByRole("menuitem", { name: "新增 terminal" })).toHaveAttribute("data-disabled");
});

it("does not create on another host if the Session changes while choosing a folder", async () => {
  let resolve!: (path: string) => void;
  mocks.choose.mockReturnValue(new Promise<string>(done => { resolve = done; }));
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getAllByRole("button", { name: "選擇資料夾並開啟 terminal" })[0]);
  await waitFor(() => expect(mocks.choose).toHaveBeenCalledOnce());
  await act(async () => { await useHerdrStore.getState().selectSession(remote); resolve("C:\\Projects\\native"); });
  expect(createSpace).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Session 已變更");
});
