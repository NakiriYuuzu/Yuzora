import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { SpaceAgentTree } from "./SpaceAgentTree";
import i18n from "@/lib/i18n";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import { useMachinesStore } from "@/state/machinesStore";
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore";
import { machine, snapshot } from "@/test/machinesFixtures";
import type { HerdrMachineAgent } from "@/lib/machinesTypes";

vi.mock("./HerdrLauncher", () => ({ HerdrLauncher: ({ viewSwitcher }: { viewSwitcher: import("react").ReactNode }) => viewSwitcher }));

const agent = (patch: Partial<HerdrMachineAgent>): HerdrMachineAgent => ({
  terminalId: "t1", paneId: "p1", tabId: "tab", workspaceId: "w", workspaceLabel: null, agent: "codex", name: "Codex",
  title: null, cwd: "/srv/app", folder: "app", status: "working", focused: false, ...patch,
});
const lab = machine("lab", { label: "Lab box" });

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.setItem("yuzora.sidebar.view", "agents");
  useHerdrStore.setState({ ...herdrInitialState, sessions: [], runtimesBySession: {}, attentionByKey: new Map() });
  useMachinesInteractiveStore.setState({ selection: null });
  useMachinesStore.getState().reset();
  useMachinesStore.setState({
    machines: [lab, machine("off", { enabled: false })],
    snapshotById: { [lab.id]: snapshot(lab.id, [agent({}), agent({ terminalId: "t2", name: null, title: "Reviewer", folder: null, status: "blocked" })]) },
  });
});
afterEach(() => { cleanup(); localStorage.clear(); });

it("lists enabled machines and their agents in the Agents view only", () => {
  render(<SpaceAgentTree />);
  expect(screen.getByText("HERDR machines")).toBeInTheDocument();
  expect(screen.getByRole("treeitem", { name: "Lab box" })).toBeInTheDocument();
  expect(screen.queryByText("Machine off")).not.toBeInTheDocument();
  const row = screen.getByRole("treeitem", { name: /^Codex · / });
  expect(row).toHaveAttribute("data-node-key", JSON.stringify(["machine", lab.id, "agent", "t1"]));
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Spaces" }), { button: 0, ctrlKey: false });
  expect(screen.queryByText("HERDR machines")).not.toBeInTheDocument();
});

it("orders tags machine then folder and omits missing values", () => {
  render(<SpaceAgentTree />);
  const full = screen.getByRole("treeitem", { name: /^Codex · / });
  expect([...full.querySelectorAll(".tree-agent-tag")].map((tag) => tag.getAttribute("data-tag"))).toEqual(["machine", "folder"]);
  expect(within(full).getByText("Lab box")).toBeInTheDocument();
  expect(within(full).getByText("app")).toBeInTheDocument();
  const partial = screen.getByRole("treeitem", { name: /^Reviewer · / });
  expect([...partial.querySelectorAll(".tree-agent-tag")].map((tag) => tag.getAttribute("data-tag"))).toEqual(["machine"]);
});

it("shows a machine as needing authentication after a failing manual status check", () => {
  useMachinesStore.setState({ statusById: { [lab.id]: { id: lab.id, label: "Lab box", status: "auth-required", error: "denied" } as never } });
  render(<SpaceAgentTree />);
  expect(screen.getByRole("treeitem", { name: /^Lab box · / })).toBeInTheDocument();
  expect(screen.queryByRole("treeitem", { name: "Lab box" })).not.toBeInTheDocument();
});

it("opens the official client with the machine label when an agent is clicked", () => {
  render(<SpaceAgentTree />);
  fireEvent.click(screen.getByRole("treeitem", { name: /^Codex · / }));
  expect(useMachinesInteractiveStore.getState().selection).toEqual({ spec: { kind: "client" }, machineLabel: "Lab box" });
});

it("hides the group when no machine is enabled", () => {
  useMachinesStore.setState({ machines: [machine("off", { enabled: false })] });
  render(<SpaceAgentTree />);
  expect(screen.queryByText("HERDR machines")).not.toBeInTheDocument();
});

it("keeps machine groups and machine agents as plain buttons that Enter/Space can activate", () => {
  render(<SpaceAgentTree />);
  for (const item of [screen.getByRole("treeitem", { name: "Lab box" }), screen.getByRole("treeitem", { name: /^Codex · / })]) {
    expect(item.tagName).toBe("BUTTON");
    item.focus();
    expect(item).toHaveFocus();
    for (const key of ["Enter", " "]) {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      item.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
  }
});

it("joins machine rows to the single roving tabindex with Arrow/Home/End navigation", () => {
  render(<SpaceAgentTree />);
  const group = screen.getByRole("treeitem", { name: "Lab box" });
  const codex = screen.getByRole("treeitem", { name: /^Codex · / });
  const reviewer = screen.getByRole("treeitem", { name: /^Reviewer · / });
  const tabbable = () => screen.getAllByRole("treeitem").filter((item) => item.getAttribute("tabindex") === "0");
  expect(tabbable()).toHaveLength(1);
  group.focus();
  expect(tabbable()).toEqual([group]);
  fireEvent.keyDown(group, { key: "ArrowDown" });
  expect(codex).toHaveFocus();
  expect(tabbable()).toEqual([codex]);
  fireEvent.keyDown(codex, { key: "ArrowDown" });
  expect(reviewer).toHaveFocus();
  fireEvent.keyDown(reviewer, { key: "ArrowDown" });
  expect(reviewer).toHaveFocus();
  fireEvent.keyDown(reviewer, { key: "ArrowUp" });
  expect(codex).toHaveFocus();
  fireEvent.keyDown(codex, { key: "Home" });
  expect(group).toHaveFocus();
  fireEvent.keyDown(group, { key: "ArrowUp" });
  expect(group).toHaveFocus();
  codex.focus();
  fireEvent.keyDown(codex, { key: "End" });
  expect(reviewer).toHaveFocus();
});

it("moves between local agent rows and machine rows with one tab stop", () => {
  const scope = '["host-a","same"]';
  useHerdrStore.setState({
    ...herdrInitialState,
    sessions: [{ name: "same", runtimeId: scope, hostId: "host-a", hostLabel: "A", running: true, default: true, sessionDir: "/", socketPath: "/sock" }],
    runtimesBySession: { [scope]: {
      capabilities: null, connectionState: "ready", errorMessage: null, worktreeInventory: null,
      snapshot: { herdrSessionId: scope, protocol: 20, version: "0.8.2", spaces: [{ id: "space", label: "Project", path: "/repo", order: 0, focused: true }],
        agents: [{ id: "agent", name: "LocalBot", status: "unknown", workspaceId: "space", paneId: "pane" }], tabs: [], terminals: [], raw: {} },
    } as never },
    selectedSessionName: scope, selectedSpaceId: "space", attentionByKey: new Map(),
  });
  render(<SpaceAgentTree />);
  const local = screen.getByRole("treeitem", { name: /^Project|LocalBot/ });
  const items = screen.getAllByRole("treeitem");
  const group = screen.getByRole("treeitem", { name: "Lab box" });
  const lastLocal = items[items.indexOf(group) - 1];
  expect(local).toBeTruthy();
  expect(items.filter((item) => item.getAttribute("tabindex") === "0")).toHaveLength(1);
  lastLocal.focus();
  fireEvent.keyDown(lastLocal, { key: "ArrowDown" });
  expect(group).toHaveFocus();
  fireEvent.keyDown(group, { key: "ArrowUp" });
  expect(lastLocal).toHaveFocus();
  fireEvent.keyDown(lastLocal, { key: "End" });
  expect(screen.getByRole("treeitem", { name: /^Reviewer · / })).toHaveFocus();
  fireEvent.keyDown(screen.getByRole("treeitem", { name: /^Reviewer · / }), { key: "Home" });
  expect(items[0]).toHaveFocus();
});

it("does not show the no-Sessions empty state next to visible machine rows", () => {
  render(<SpaceAgentTree />);
  expect(screen.getByRole("treeitem", { name: "Lab box" })).toBeInTheDocument();
  expect(screen.queryByText("No running Sessions.")).not.toBeInTheDocument();
});
