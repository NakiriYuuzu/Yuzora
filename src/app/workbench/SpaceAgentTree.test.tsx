import { useContextMenuStore } from "@/state/contextMenuStore";

import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SpaceAgentTree } from "./SpaceAgentTree";
import { useHerdrStore, herdrInitialState } from "@/state/herdrStore";
import { spacePresentationKey } from "./spaceTreeIdentity";
import {
  loadRecentWorkspacePresentations,
  useRecentWorkspacesStore,
} from "@/state/recentWorkspaces";
import type { HerdrSnapshot, HerdrSessionRuntime } from "@/lib/herdrTypes";
vi.mock("./HerdrLauncher", () => ({ HerdrLauncher: () => null }));
vi.mock("./HerdrAgentInspector", () => ({
  HerdrAgentInspector: ({
    open,
    agent,
  }: {
    open: boolean;
    agent: { sessionName: string } | null;
  }) => (open ? <div data-testid="inspector">{agent?.sessionName}</div> : null),
}));
const scopes = [
  '["host-a","same"]',
  '["host-b","same"]',
  '["host-a","second"]',
];
beforeEach(() => {
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    },
  });
  useRecentWorkspacesStore.setState({ presentations: {} });
  const runtimes: Record<string, HerdrSessionRuntime> = {};
  for (const scope of scopes)
    runtimes[scope] = {
      capabilities: null,
      connectionState: "ready",
      errorMessage: null,
      worktreeInventory: null,
      snapshot: {
        herdrSessionId: scope,
        protocol: 20,
        version: "0.8.2",
        spaces: [
          {
            id: "space",
            label: "Project",
            path: "/repo",
            order: 0,
            focused: true,
          },
        ],
        agents: [
          {
            id: "agent",
            name: "Codex",
            status: "unknown",
            workspaceId: "space",
            paneId: "pane",
          },
        ],
        tabs: [],
        terminals: [],
        raw: {},
      } as HerdrSnapshot,
    };
  useHerdrStore.setState({
    ...herdrInitialState,
    sessions: scopes.map((scope, i) => ({
      name: i === 2 ? "second" : "same",
      runtimeId: scope,
      hostId: i === 1 ? "host-b" : "host-a",
      hostLabel: i === 1 ? "B" : "A",
      running: true,
      default: false,
      sessionDir: "/",
      socketPath: "/sock",
    })),
    runtimesBySession: runtimes,
    selectedSessionName: scopes[0],
    selectedSpaceId: "space",
    attentionByKey: new Map(),
  });
});
afterEach(cleanup);
it("saves first-seen Bot combinations and reuses them after the tree remounts", () => {
  const firstMount = render(<SpaceAgentTree />);
  const saved = loadRecentWorkspacePresentations();
  const key = spacePresentationKey(scopes[0], "/repo");
  // Two hosts own two identities; the first host's Sessions share one Bot.
  expect(Object.keys(saved)).toHaveLength(2);
  expect(saved[key].character).toBeDefined();
  const appearances = (container: HTMLElement) =>
    [...container.querySelectorAll(".tree-space-identity .space-character-art")].map((art) => [
      art.getAttribute("data-shell"), art.getAttribute("data-face"), art.getAttribute("data-detail"),
    ]);
  const before = appearances(firstMount.container);
  expect(before).toHaveLength(3);
  expect(before[0]).toEqual(before[2]);
  firstMount.unmount();
  useRecentWorkspacesStore.setState({ presentations: saved });

  const secondMount = render(<SpaceAgentTree />);

  expect(appearances(secondMount.container)).toEqual(before);
  expect(loadRecentWorkspacePresentations()).toEqual(saved);
});
it("keeps same path and agent IDs separate across hosts and sessions", () => {
  render(<SpaceAgentTree />);
  const leaves = screen
    .getAllByRole("treeitem")
    .filter((x) => x.getAttribute("aria-level") === "3");
  expect(leaves).toHaveLength(3);
  fireEvent.click(leaves[1]);
  expect(screen.getByTestId("inspector")).toHaveTextContent(scopes[1]);
  expect(useHerdrStore.getState().selectedSessionName).toBe(scopes[0]);
});

it("removes stopped sessions and their cached projects from the sidebar", () => {
  useHerdrStore.setState({ sessions: useHerdrStore.getState().sessions.map((session, i) => ({ ...session, running: i !== 1 })) });
  render(<SpaceAgentTree />);
  expect(screen.getAllByRole("treeitem").filter(row => row.getAttribute("aria-level") === "3")).toHaveLength(2);
  expect(screen.queryAllByRole("treeitem").some(row => row.getAttribute("aria-label")?.includes(scopes[1]))).toBe(false);
});
it("supports Home, End, parent navigation and collapse without changing runtime selection", () => {
  render(<SpaceAgentTree />);
  const rows = screen.getAllByRole("treeitem");
  rows[0].focus();
  fireEvent.keyDown(rows[0], { key: "End" });
  expect(document.activeElement).toBe(rows.at(-1));
  fireEvent.keyDown(rows.at(-1)!, { key: "ArrowLeft" });
  expect(document.activeElement).toBe(rows.at(-2));
  fireEvent.keyDown(document.activeElement!, { key: "Home" });
  expect(document.activeElement).toBe(rows[0]);
  fireEvent.keyDown(rows[0], { key: "ArrowLeft" });
  expect(screen.getAllByRole("treeitem")).toHaveLength(7);
  expect(useHerdrStore.getState().selectedSessionName).toBe(scopes[0]);
});
it("collapses and expands only the requested Session, including same-named Sessions on different hosts", () => {
  render(<SpaceAgentTree />);
  expect(screen.getAllByRole("button", { name: /^Collapse Spaces in / })).toHaveLength(3);
  expect(screen.queryByText("Loaded snapshot")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Collapse Spaces in B · same" }));
  expect(screen.getAllByRole("treeitem")).toHaveLength(7);
  const expand = screen.getByRole("button", { name: "Expand Spaces in B · same" });
  expect(expand).toHaveAttribute("aria-expanded", "false");
  expect(screen.getByRole("button", { name: "Collapse Spaces in A · same" })).toHaveAttribute("aria-expanded", "true");
  expect(useHerdrStore.getState().selectedSessionName).toBe(scopes[0]);
  fireEvent.click(expand);
  expect(screen.getAllByRole("treeitem")).toHaveLength(9);
  expect(screen.getByRole("button", { name: "Collapse Spaces in B · same" })).toHaveAttribute("aria-expanded", "true");
  const project = screen.getAllByRole("treeitem").filter(row => row.getAttribute("aria-level") === "1")[1];
  fireEvent.keyDown(project, { key: "ArrowLeft" });
  fireEvent.click(screen.getByRole("button", { name: "Expand Spaces in B · same" }));
  expect(screen.getAllByRole("treeitem")).toHaveLength(9);
});
it("persists sanitized static character preference scoped by host, shared only between that host sessions", () => {
  const first = spacePresentationKey(scopes[0], "/repo"),
    second = spacePresentationKey(scopes[1], "/repo");
  expect(first).not.toBe(second);
  expect(first).toBe(spacePresentationKey(scopes[2], "/repo"));
  useRecentWorkspacesStore.getState().updatePresentation(first, {
    name: "A",
    avatarMode: "character",
    character: {
      shell: "cloud",
      face: "curious",
      detail: "none",
      motion: false,
    },
  });
  const saved = loadRecentWorkspacePresentations();
  expect(saved[first].character?.motion).toBe(false);
  expect(saved[second]).toBeUndefined();
});
it("F2 opens host-scoped appearance editor without activating a runtime", () => {
  render(<SpaceAgentTree />);
  const projects = screen
    .getAllByRole("treeitem")
    .filter((x) => x.getAttribute("aria-level") === "1");
  fireEvent.keyDown(projects[1], { key: "F2" });
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(useHerdrStore.getState().selectedSessionName).toBe(scopes[0]);
});

it("offers ready agents an independent Inspector action without activation", () => {
  const activateAgent = vi.fn();
  const runtime = useHerdrStore.getState().runtimesBySession[scopes[1]];
  useHerdrStore.setState({
    activateAgent,
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      [scopes[1]]: {
        ...runtime,
        connectionState: "ready",
        capabilities: {
          server: { running: true },
          api: { workspaceFocus: true, tabFocus: true },
        } as HerdrSessionRuntime["capabilities"],
        snapshot: {
          ...runtime.snapshot!,
          agents: runtime.snapshot!.agents.map((agent) => ({
            ...agent,
            terminalId: "terminal",
          })),
        },
      },
    },
  });
  render(<SpaceAgentTree />);
  const leaf = screen
    .getAllByRole("treeitem")
    .filter((item) => item.getAttribute("aria-level") === "3")[1];
  fireEvent.focus(leaf);
  const inspect = screen.getAllByRole("button", {
    name: "Inspect Agent: Codex",
  })[1];
  expect(inspect).toHaveAttribute("tabindex", "0");
  fireEvent.click(inspect);
  expect(screen.getByTestId("inspector")).toHaveTextContent(scopes[1]);
  expect(activateAgent).not.toHaveBeenCalled();
});

it("dispatches worktree and agent menus with their exact host namespace", () => {
  render(<SpaceAgentTree />);
  const rows = screen.getAllByRole("treeitem");
  fireEvent.contextMenu(
    rows.filter((item) => item.getAttribute("aria-level") === "2")[1],
  );
  expect(useContextMenuStore.getState().request).toMatchObject({
    kind: "herdrSpace",
    sessionName: scopes[1],
    workspaceId: "space",
    path: "/repo",
  });
  fireEvent.contextMenu(
    rows.filter((item) => item.getAttribute("aria-level") === "3")[2],
  );
  expect(useContextMenuStore.getState().request).toMatchObject({
    kind: "herdrPane",
    sessionName: scopes[2],
    workspaceId: "space",
    paneId: "pane",
  });
});

it.each([undefined, null])("uses the workspace label for an agent without a repository branch (%s)", (branch) => {
  const runtimes = useHerdrStore.getState().runtimesBySession;
  const runtime = runtimes[scopes[0]];
  useHerdrStore.setState({
    runtimesBySession: {
      ...runtimes,
      [scopes[0]]: {
        ...runtime,
        snapshot: {
          ...runtime.snapshot!,
          spaces: [{ ...runtime.snapshot!.spaces[0], label: "QA-A", path: "/tmp/QA-A", branch }],
        },
      },
    },
  });
  render(<SpaceAgentTree />);
  const agent = screen.getAllByRole("treeitem").find((row) => row.getAttribute("aria-level") === "3")!;
  expect(agent).toHaveAccessibleName(`Codex · Codex · Unknown · QA-A · ${scopes[0]}`);
  expect(agent.getAttribute("aria-label")).not.toMatch(/undefined|null/);
});
