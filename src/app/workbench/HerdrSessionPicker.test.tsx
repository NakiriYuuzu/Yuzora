import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { HerdrSessionPicker } from "./HerdrSessionPicker";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import { useHostStore } from "@/state/hostStore";
import { useSshStore, type SshHost } from "@/state/sshStore";
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore";
import { useUiStore } from "@/state/uiStore";
import type { HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes";
import type { HostTarget } from "@/lib/hostIpc";
import i18n from "@/lib/i18n";

const mocks = vi.hoisted(() => ({ windows: false, distros: vi.fn(), setup: vi.fn(), refresh: vi.fn(), select: vi.fn() }));
vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isWindowsPlatform: () => mocks.windows }));
vi.mock("@/lib/hostIpc", async original => ({ ...await original<typeof import("@/lib/hostIpc")>(), wslDistributions: mocks.distros }));

const originalHerdr = useHerdrStore.getState();
const originalHosts = useHostStore.getState();
const originalSsh = useSshStore.getState();
const originalWsl = useRuntimePreferencesStore.getState().wslEnabled;
const onSelect = vi.fn();
const onClose = vi.fn();
const alpha: SshHost = { id: "alpha", name: "Alpha", host: "alpha.example.invalid", port: 22, user: "tester", authKind: "password" };
function addSession(hostId: string, label: string) {
  const scope = JSON.stringify([hostId, "default"]);
  const runtime = {
    connectionState: "ready", errorMessage: null, worktreeInventory: null,
    capabilities: { server: { running: true, compatible: true } },
    snapshot: { herdrSessionId: scope, spaces: [], agents: [], tabs: [], terminals: [], raw: {} } as unknown as HerdrSnapshot,
  } as HerdrSessionRuntime;
  useHerdrStore.setState(state => ({
    sessions: [...state.sessions.filter(session => session.runtimeId !== scope), { name: "default", runtimeId: scope, hostId, hostLabel: label, running: true, default: false, sessionDir: "/fixture", socketPath: "/fixture/socket" }],
    runtimesBySession: { ...state.runtimesBySession, [scope]: runtime },
  }));
  return scope;
}
function connectSsh(host = alpha) {
  useSshStore.setState({
    activeHostId: host.id,
    sessions: { [host.id]: { hostId: host.id, sessionId: `transport-${host.id}`, status: "connected", fingerprint: "fixture", knownHost: true, error: null } },
  });
}
function registerHost(hostId: string, target: HostTarget) {
  const connection = { owner: { hostId, generation: 1 }, hello: { protocol: 1, version: "fixture", os: "linux", arch: "aarch64", home: "/home/tester", methods: [] } };
  useHostStore.setState(state => ({ hosts: { ...state.hosts, [hostId]: { connection, target, connecting: false, error: null, attempt: 0, retryAt: 0 } } }));
  return connection;
}
beforeEach(async () => {
  vi.resetAllMocks();
  mocks.windows = false;
  await i18n.changeLanguage("zh-TW");
  useHerdrStore.setState({ ...herdrInitialState, refreshSessions: mocks.refresh, selectSession: mocks.select });
  useHostStore.setState({ hosts: {}, configs: {}, setup: mocks.setup });
  useSshStore.setState({ hosts: [alpha], sessions: {}, activeHostId: null, pendingAuthHostId: null });
  useRuntimePreferencesStore.setState({ wslEnabled: false });
  mocks.refresh.mockResolvedValue(undefined);
  mocks.distros.mockResolvedValue([]);
  mocks.select.mockImplementation(async (scope: string) => useHerdrStore.setState({ selectedSessionName: scope }));
  mocks.setup.mockImplementation(async (hostId: string, label: string, target: HostTarget) => {
    const connection = registerHost(hostId, target);
    addSession(hostId, label);
    return connection;
  });
});
afterEach(() => {
  cleanup();
  useHerdrStore.setState(originalHerdr);
  useHostStore.setState(originalHosts);
  useSshStore.setState(originalSsh);
  useRuntimePreferencesStore.setState({ wslEnabled: originalWsl });
});
async function show(source: "SSH 遠端主機" | "WSL" = "SSH 遠端主機") {
  const view = render(<HerdrSessionPicker initialSession={null} onSelect={onSelect} onClose={onClose} returnFocusRef={{ current: null }} />);
  const tab = screen.getByRole("tab", { name: source });
  await waitFor(() => expect(tab).toBeEnabled());
  fireEvent.mouseDown(tab, { button: 0, ctrlKey: false });
  return view;
}

it("reuses the host book and password authentication flow for an unconnected SSH host", async () => {
  await show();
  fireEvent.click(screen.getByRole("button", { name: /Alpha\s*tester@alpha/ }));
  expect(useSshStore.getState().pendingAuthHostId).toBe("alpha");
  expect(screen.getByRole("button", { name: "載入 Session" })).toBeDisabled();
  expect(mocks.setup).not.toHaveBeenCalled();
});

it("can add a new SSH host from inside the Session picker", async () => {
  useSshStore.setState({ hosts: [] });
  await show();
  fireEvent.click(screen.getByRole("button", { name: "新增主機" }));
  const dialog = screen.getByRole("dialog", { name: "新增 SSH 主機" });
  fireEvent.change(within(dialog).getByRole("textbox", { name: "名稱" }), { target: { value: "Beta" } });
  fireEvent.change(within(dialog).getByRole("textbox", { name: "主機" }), { target: { value: "beta.example.invalid" } });
  fireEvent.change(within(dialog).getByRole("textbox", { name: "使用者" }), { target: { value: "tester" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "新增" }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "新增 SSH 主機" })).not.toBeInTheDocument());
  const host = useSshStore.getState().hosts[0];
  expect(host).toMatchObject({ name: "Beta", host: "beta.example.invalid", user: "tester", authKind: "password" });
  fireEvent.click(screen.getByRole("button", { name: /Beta\s*tester@beta/ }));
  expect(useSshStore.getState().pendingAuthHostId).toBe(host.id);
});

it("sets up the selected SSH host, refreshes Sessions, and loads its exact namespace", async () => {
  connectSsh();
  addSession("other", "Other host");
  await show();
  expect(screen.getByRole("button", { name: "載入 Session" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "設定此主機" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "載入 Session" })).toBeEnabled());
  expect(mocks.setup).toHaveBeenCalledWith("alpha", "Alpha", { kind: "ssh", sessionId: "transport-alpha" }, { source: "default" });
  expect(mocks.refresh).toHaveBeenCalledTimes(2);
  const picker = screen.getByRole("combobox", { name: "所屬 Herdr Session" });
  expect(picker).toHaveTextContent("Alpha · default");
  fireEvent.keyDown(picker, { key: "ArrowDown" });
  expect(screen.queryByRole("option", { name: "Other host · default" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("option", { name: "Alpha · default" }));
  fireEvent.click(screen.getByRole("button", { name: "載入 Session" }));
  await waitFor(() => expect(onSelect).toHaveBeenCalledWith('["alpha","default"]'));
  expect(mocks.select).toHaveBeenCalledWith('["alpha","default"]');
});

it("loads a connected runtime without redeploying its host tools", async () => {
  connectSsh();
  registerHost("alpha", { kind: "ssh", sessionId: "transport-alpha" });
  addSession("alpha", "Alpha");
  await show();
  expect(screen.queryByRole("button", { name: "設定此主機" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "載入 Session" }));
  await waitFor(() => expect(onSelect).toHaveBeenCalledWith('["alpha","default"]'));
  expect(mocks.setup).not.toHaveBeenCalled();
});

it("keeps incompatible setup errors visible and does not load cached Sessions from that host", async () => {
  connectSsh();
  addSession("alpha", "Alpha");
  mocks.setup.mockRejectedValue(new Error("runtime-incompatible"));
  await show();
  fireEvent.click(screen.getByRole("button", { name: "設定此主機" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("runtime-incompatible"));
  expect(screen.getByRole("button", { name: "載入 Session" })).toBeDisabled();
  expect(mocks.select).not.toHaveBeenCalled();
});

it("does not use a runtime connection from a previous SSH transport", async () => {
  connectSsh();
  registerHost("alpha", { kind: "ssh", sessionId: "old-transport" });
  addSession("alpha", "Alpha");
  await show();
  expect(screen.getByRole("button", { name: "載入 Session" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "設定此主機" })).toBeEnabled();
});

it("keeps WSL discovery and connection gated by the existing opt-in setting", async () => {
  mocks.windows = true;
  await show("WSL");
  expect(screen.getByText("請先到設定 → HERDR 啟用 WSL。")).toBeInTheDocument();
  expect(mocks.distros).not.toHaveBeenCalled();
  expect(mocks.setup).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "HERDR 設定" }));
  expect(onClose).toHaveBeenCalledOnce();
  expect(useUiStore.getState().settingsSection).toBe("herdr");
});

it("discovers WSL2, connects the selected distribution and loads its Session", async () => {
  mocks.windows = true;
  useRuntimePreferencesStore.setState({ wslEnabled: true });
  mocks.distros.mockResolvedValue([{ hostId: "wsl:legacy", name: "Legacy", version: 1 }, { hostId: "wsl:ubuntu", name: "Ubuntu", version: 2 }]);
  await show("WSL");
  await waitFor(() => expect(screen.getByRole("combobox", { name: "WSL 發行版" })).toHaveTextContent("Ubuntu · WSL2"));
  fireEvent.keyDown(screen.getByRole("combobox", { name: "WSL 發行版" }), { key: "ArrowDown" });
  expect(screen.getByRole("option", { name: "Legacy · WSL1" })).toHaveAttribute("aria-disabled", "true");
  fireEvent.click(screen.getByRole("option", { name: "Ubuntu · WSL2" }));
  fireEvent.click(screen.getByRole("button", { name: "設定此主機" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "載入 Session" })).toBeEnabled());
  expect(mocks.setup).toHaveBeenCalledWith("wsl:ubuntu", "Ubuntu", { kind: "wsl", distro: "Ubuntu" }, { source: "default" });
  fireEvent.click(screen.getByRole("button", { name: "載入 Session" }));
  await waitFor(() => expect(onSelect).toHaveBeenCalledWith('["wsl:ubuntu","default"]'));
});

it("does not show or discover WSL on non-Windows hosts", async () => {
  useRuntimePreferencesStore.setState({ wslEnabled: true });
  await show();
  expect(screen.queryByRole("tab", { name: "WSL" })).not.toBeInTheDocument();
  expect(mocks.distros).not.toHaveBeenCalled();
});

it("does not apply a delayed selection after the picker closes", async () => {
  connectSsh();
  registerHost("alpha", { kind: "ssh", sessionId: "transport-alpha" });
  addSession("alpha", "Alpha");
  let resolve!: () => void;
  mocks.select.mockReturnValue(new Promise<void>(done => { resolve = done; }));
  const view = await show();
  fireEvent.click(screen.getByRole("button", { name: "載入 Session" }));
  view.unmount();
  await act(async () => resolve());
  expect(onSelect).not.toHaveBeenCalled();
});
