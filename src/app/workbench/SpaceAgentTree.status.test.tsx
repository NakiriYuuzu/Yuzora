import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SpaceAgentTree } from "./SpaceAgentTree";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import type { HerdrCapabilities, HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes";
import i18n from "@/lib/i18n";

const scope = '["wsl:{test-distro}:1000","default"]';
vi.mock("./HerdrLauncher", () => ({
  HerdrLauncher: ({ onScopeChange }: { onScopeChange: (name: string) => void }) =>
    <button onClick={() => onScopeChange('["wsl:{test-distro}:1000","default"]')}>select session</button>,
}));
vi.mock("./HerdrAgentInspector", () => ({ HerdrAgentInspector: () => null }));
beforeEach(async () => {
  await i18n.changeLanguage("zh-TW");
  useHerdrStore.setState({ ...herdrInitialState, sessions: [{ name: "default", runtimeId: scope, hostId: "wsl:{test-distro}:1000", hostLabel: "Ubuntu-24.04", running: true, default: true, sessionDir: "/fixture", socketPath: "/fixture/socket" }], selectedSessionName: scope });
});
afterEach(() => { cleanup(); useHerdrStore.setState(herdrInitialState); });
function show(connectionState: HerdrSessionRuntime["connectionState"], snapshot: HerdrSnapshot | null, errorMessage: string | null, capabilities: HerdrCapabilities | null = null) {
  useHerdrStore.setState({ runtimesBySession: { [scope]: { connectionState, snapshot, errorMessage, capabilities, worktreeInventory: null } } });
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getByText("select session"));
}
const emptySnapshot = { herdrSessionId: scope, protocol: 22, version: "0.9.0", spaces: [], agents: [], tabs: [], terminals: [], raw: {} } as HerdrSnapshot;

it("shows incompatible client and server identities without claiming loaded or empty data", () => {
  show("unsupported", null, "herdr server protocol incompatible", {
    binaryPath: "/managed/herdr", binaryVersion: "0.8.2", binaryProtocol: 20,
    server: { running: true, compatible: false, version: "0.9.0", protocol: 22 },
  } as HerdrCapabilities);
  expect(screen.queryByText("已載入快照")).not.toBeInTheDocument();
  expect(screen.queryByText(/此 Session 尚無 Space/)).not.toBeInTheDocument();
  expect(document.body).toHaveTextContent("0.8.2");
  expect(document.body).toHaveTextContent("0.9.0");
  expect(document.body).toHaveTextContent("protocol 20");
  expect(document.body).toHaveTextContent("protocol 22");
  expect(document.body).toHaveTextContent("/managed/herdr");
  expect(document.body).not.toHaveTextContent(scope);
});
it.each(["idle", "connecting"] as const)("does not describe %s as an empty successful snapshot", state => {
  show(state, null, null);
  expect(document.body).toHaveTextContent("載入中");
  expect(screen.queryByText("已載入快照")).not.toBeInTheDocument();
  expect(screen.queryByText(/此 Session 尚無 Space/)).not.toBeInTheDocument();
});
it("shows a successful empty snapshot with a readable host and Session name", () => {
  show("ready", emptySnapshot, null);
  expect(screen.getByText("已載入快照")).toBeInTheDocument();
  expect(document.body).toHaveTextContent("Ubuntu-24.04 · default: 此 Session 尚無 Space。");
  expect(document.body).not.toHaveTextContent(scope);
});
it("marks cached snapshots stale after a refresh error", () => {
  show("ready", emptySnapshot, "socket disconnected");
  expect(document.body).toHaveTextContent("資料尚未更新");
  expect(document.body).toHaveTextContent("socket disconnected");
  expect(screen.queryByText("已載入快照")).not.toBeInTheDocument();
  expect(screen.queryByText(/此 Session 尚無 Space/)).not.toBeInTheDocument();
});
it("shows the error rather than an empty snapshot when no data could be read", () => {
  show("error", null, "socket disconnected");
  expect(document.body).toHaveTextContent("socket disconnected");
  expect(screen.queryByText("已載入快照")).not.toBeInTheDocument();
  expect(screen.queryByText(/此 Session 尚無 Space/)).not.toBeInTheDocument();
});
