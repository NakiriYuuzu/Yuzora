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

it("keeps machine groups and machine agents as plain tabbable buttons that Enter/Space can activate", () => {
  render(<SpaceAgentTree />);
  for (const item of [screen.getByRole("treeitem", { name: "Lab box" }), screen.getByRole("treeitem", { name: /^Codex · / })]) {
    expect(item.tagName).toBe("BUTTON");
    expect(item).toHaveAttribute("tabindex", "0");
    item.focus();
    expect(item).toHaveFocus();
    for (const key of ["Enter", " "]) {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      item.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
    }
  }
});
