import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { HerdrSessionPicker } from "./HerdrSessionPicker";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import { useSshStore, type SshHost } from "@/state/sshStore";
import { useMachinesStore } from "@/state/machinesStore";
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore";
import { useUiStore } from "@/state/uiStore";
import { machine, supportedCaps } from "@/test/machinesFixtures";
import i18n from "@/lib/i18n";

const ipc = vi.hoisted(() => ({ caps: vi.fn(), list: vi.fn(), windows: false }));
vi.mock("@/lib/machinesIpc", () => ({
  machinesCapabilities: ipc.caps, machinesList: ipc.list, machinesStatus: vi.fn(), machinesAgents: vi.fn(),
  machinesRename: vi.fn(), machinesSetEnabled: vi.fn(), machinesRemove: vi.fn(),
}));
vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isWindowsPlatform: () => ipc.windows }));

const onClose = vi.fn();
const alpha: SshHost = { id: "alpha", name: "Alpha", host: "alpha.example.invalid", port: 22, user: "tester", authKind: "password" };

async function show(tab: string) {
  render(<HerdrSessionPicker initialSession={null} onSelect={vi.fn()} onClose={onClose} returnFocusRef={{ current: null }} />);
  const trigger = screen.getByRole("tab", { name: new RegExp(tab) });
  await waitFor(() => expect(trigger).toBeEnabled());
  fireEvent.mouseDown(trigger, { button: 0, ctrlKey: false });
}

beforeEach(async () => {
  vi.resetAllMocks();
  ipc.windows = false;
  await i18n.changeLanguage("zh-TW");
  useHerdrStore.setState({ ...herdrInitialState, refreshSessions: vi.fn().mockResolvedValue(undefined) });
  useSshStore.setState({ hosts: [alpha], sessions: {}, activeHostId: null, pendingAuthHostId: null });
  useMachinesStore.getState().reset();
  useMachinesInteractiveStore.setState({ selection: null });
  ipc.caps.mockResolvedValue(supportedCaps);
  ipc.list.mockResolvedValue([machine("lab", { label: "Lab box", target: "me@lab" })]);
});
afterEach(cleanup);

it("adds a Machines tab that lists the saved machines", async () => {
  await show("HERDR machines");
  expect(await screen.findByText("Lab box")).toBeInTheDocument();
  expect(screen.getByText("me@lab · default")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "載入 Session" })).not.toBeInTheDocument();
});

it("explains the version requirement and links to HERDR settings when machines are unsupported", async () => {
  ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false, version: "0.9.1", reason: "machines-runtime-too-old" });
  const openSettings = vi.spyOn(useUiStore.getState(), "openSettings").mockImplementation(() => undefined);
  await show("HERDR machines");
  expect(await screen.findByText(/需要 HERDR 0\.9\.2 以上/)).toBeInTheDocument();
  expect(ipc.list).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "前往 HERDR 設定" }));
  expect(openSettings).toHaveBeenCalledWith("herdr");
  expect(onClose).toHaveBeenCalled();
});

it("shows list failures with a retry without breaking the other tabs", async () => {
  ipc.list.mockRejectedValueOnce("machines-binary-unavailable");
  await show("HERDR machines");
  expect(await screen.findByText("無法載入 machines")).toBeInTheDocument();
  ipc.list.mockResolvedValue([machine("lab", { label: "Lab box" })]);
  fireEvent.click(screen.getByRole("button", { name: "重試" }));
  expect(await screen.findByText("Lab box")).toBeInTheDocument();
});

it("starts the interactive add flow from the add dialog and closes the picker", async () => {
  await show("HERDR machines");
  fireEvent.click(await screen.findByRole("button", { name: "新增 machine" }));
  const dialog = await screen.findByRole("dialog", { name: "新增 HERDR machine" });
  fireEvent.change(within(dialog).getByLabelText("SSH target"), { target: { value: "me@new-host" } });
  fireEvent.change(within(dialog).getByLabelText("名稱（選填）"), { target: { value: "New" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "繼續" }));
  expect(useMachinesInteractiveStore.getState().selection).toEqual({ spec: { kind: "add", target: "me@new-host", label: "New" } });
  expect(onClose).toHaveBeenCalled();
});

it("marks the SSH tab as the legacy path", async () => {
  await show("SSH");
  expect(screen.getByText("舊路徑")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "轉成 HERDR machine" })).toBeInTheDocument();
});

it.each([
  [{ ...alpha }, "tester@alpha.example.invalid"],
  [{ ...alpha, port: 2222 }, "ssh://tester@alpha.example.invalid:2222"],
  [{ ...alpha, host: "fe80::1", port: 22 }, "ssh://tester@[fe80::1]"],
])("prefills the migration target and label for %j", async (host, expected) => {
  useSshStore.setState({ hosts: [host] });
  await show("SSH");
  fireEvent.click(screen.getByRole("button", { name: "轉成 HERDR machine" }));
  const dialog = await screen.findByRole("dialog", { name: "將 Alpha 轉成 HERDR machine" });
  expect(within(dialog).getByLabelText("SSH target")).toHaveValue(expected);
  expect(within(dialog).getByLabelText("名稱（選填）")).toHaveValue("Alpha");
  fireEvent.click(within(dialog).getByRole("button", { name: "繼續" }));
  expect(useMachinesInteractiveStore.getState().selection).toEqual({ spec: { kind: "add", target: expected, label: "Alpha" } });
  expect(useSshStore.getState().hosts).toEqual([host]);
});

it("warns that key files are not passed on", async () => {
  useSshStore.setState({ hosts: [{ ...alpha, authKind: "key", keyPath: "/k" }] });
  await show("SSH");
  fireEvent.click(screen.getByRole("button", { name: "轉成 HERDR machine" }));
  expect(await screen.findByText(/不接受金鑰檔/)).toBeInTheDocument();
});
