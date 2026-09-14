import { useContextMenuStore } from "@/state/contextMenuStore";

import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SpaceAgentTree } from "./SpaceAgentTree";
import { useHerdrStore, herdrInitialState } from "@/state/herdrStore";
import { spacePresentationKey } from "./spaceTreeIdentity";
import { herdrWorkspaceMove, herdrWorkspaceMoveBlock } from "@/lib/herdrIpc";
import {
  loadRecentWorkspacePresentations,
  useRecentWorkspacesStore,
} from "@/state/recentWorkspaces";
import type { HerdrSnapshot, HerdrSessionRuntime } from "@/lib/herdrTypes";
vi.mock("@/lib/herdrIpc", async () => ({
  ...(await vi.importActual<typeof import("@/lib/herdrIpc")>("@/lib/herdrIpc")),
  herdrWorkspaceMove: vi.fn(),
  herdrWorkspaceMoveBlock: vi.fn(),
}));
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
  vi.clearAllMocks();
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

it("reorders Spaces through Herdr workspace.move", async () => {
  const sessionName = scopes[0];
  const runtime = useHerdrStore.getState().runtimesBySession[sessionName];
  useHerdrStore.setState({
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      [sessionName]: {
        ...runtime,
        capabilities: {
          server: { running: true, compatible: true },
          api: { workspaceMove: true },
        } as HerdrSessionRuntime["capabilities"],
        snapshot: {
          ...runtime.snapshot!,
          spaces: [
            { id: "space-a", label: "A", branch: "A", repoKey: "repo-a", repoRoot: "/repo-a", path: "/repo-a", order: 0, focused: true },
            { id: "space-b", label: "B", branch: "B", repoKey: "repo-b", repoRoot: "/repo-b", path: "/repo-b", order: 1, focused: false },
          ],
        },
      },
    },
  });
  render(<SpaceAgentTree />);
  const worktrees = screen
    .getAllByRole("treeitem")
    .filter((item) => item.getAttribute("aria-level") === "1");
  const [target, source] = worktrees;
  const dataTransfer = {
    effectAllowed: "none",
    dropEffect: "none",
    setData: vi.fn(),
    getData: () => "space-b",
  };
  fireEvent.dragStart(source, { dataTransfer });
  fireEvent.dragOver(target, { dataTransfer, clientY: 0 });
  fireEvent.drop(target, { dataTransfer, clientY: 0 });
  await vi.waitFor(() => expect(herdrWorkspaceMove).toHaveBeenCalledWith({
    sessionName,
    workspaceId: "space-b",
    insertIndex: 0,
  }));
  expect(source).toHaveAttribute("draggable", "false");
  expect(screen.getAllByRole("treeitem").filter((item) => item.getAttribute("aria-level") === "2")[0]).toHaveAttribute("draggable", "false");
});

it("reorders a WSL Space through HERDR workspace.move_block when legacy move is absent", async () => {
  const sessionName = scopes[0];
  const runtime = useHerdrStore.getState().runtimesBySession[sessionName];
  useHerdrStore.setState({
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      [sessionName]: {
        ...runtime,
        capabilities: {
          server: { running: true, compatible: true },
          api: { workspaceMoveBlock: true, methods: ["workspace.move_block"] },
        } as HerdrSessionRuntime["capabilities"],
        snapshot: {
          ...runtime.snapshot!,
          spaces: [
            { id: "space-a", label: "A", branch: "A", repoKey: "repo-a", repoRoot: "/repo-a", path: "/repo-a", order: 0, focused: true },
            { id: "space-b", label: "B", branch: "B", repoKey: "repo-b", repoRoot: "/repo-b", path: "/repo-b", order: 1, focused: false },
          ],
        },
      },
    },
  });
  render(<SpaceAgentTree />);
  const worktrees = screen.getAllByRole("treeitem").filter((item) => item.getAttribute("aria-level") === "1");
  const [target, source] = worktrees;
  const dataTransfer = { effectAllowed: "none", dropEffect: "none", setData: vi.fn(), getData: () => "space-b" };
  fireEvent.dragStart(source, { dataTransfer });
  fireEvent.dragOver(target, { dataTransfer, clientY: 0 });
  fireEvent.drop(target, { dataTransfer, clientY: 0 });
  await vi.waitFor(() => expect(herdrWorkspaceMoveBlock).toHaveBeenCalledOnce());
  const blockRequest = vi.mocked(herdrWorkspaceMoveBlock).mock.calls[0][0];
  expect(blockRequest.sessionName).toBe(sessionName);
  expect(blockRequest.workspaceIds).toHaveLength(1);
  expect(blockRequest.beforeWorkspaceId).toBeTruthy();
  expect(blockRequest.beforeWorkspaceId).not.toBe(blockRequest.workspaceIds[0]);
  expect(herdrWorkspaceMove).not.toHaveBeenCalled();
});

it("keeps a normal Space click after a pointer press that does not cross the drag threshold", () => {
  const sessionName = scopes[0];
  const runtime = useHerdrStore.getState().runtimesBySession[sessionName];
  useHerdrStore.setState({
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      [sessionName]: {
        ...runtime,
        capabilities: {
          server: { running: true, compatible: true },
          api: { workspaceMove: true },
        } as HerdrSessionRuntime["capabilities"],
      },
    },
  });
  render(<SpaceAgentTree />);
  const space = screen
    .getAllByRole("treeitem")
    .find((item) => item.getAttribute("aria-level") === "1")!;
  const button = space;
  fireEvent.pointerDown(button, { button: 0, pointerId: 7, clientX: 10, clientY: 10 });
  fireEvent.pointerUp(button, { button: 0, pointerId: 7, clientX: 10, clientY: 10 });
  fireEvent.click(button);
  expect(useHerdrStore.getState().selectedSpaceId).toBe("space");
  expect(herdrWorkspaceMove).not.toHaveBeenCalled();
});

it("keeps WSL Space reordering alive when pointer capture is unavailable", async () => {
  const sessionName = scopes[0];
  const runtime = useHerdrStore.getState().runtimesBySession[sessionName];
  useHerdrStore.setState({
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      [sessionName]: {
        ...runtime,
        capabilities: {
          server: { running: true, compatible: true },
          api: { workspaceMoveBlock: true, methods: ["workspace.move_block"] },
        } as HerdrSessionRuntime["capabilities"],
        snapshot: {
          ...runtime.snapshot!,
          spaces: [
            { id: "space-a", label: "A", branch: "A", repoKey: "repo-a", repoRoot: "/repo-a", path: "/repo-a", order: 0, focused: true },
            { id: "space-b", label: "B", branch: "B", repoKey: "repo-b", repoRoot: "/repo-b", path: "/repo-b", order: 1, focused: false },
          ],
        },
      },
    },
  });
  const originalSetPointerCapture = HTMLElement.prototype.setPointerCapture;
  const setPointerCapture = vi.fn(() => {
    throw new DOMException("pointer capture unavailable", "NotFoundError");
  });
  Object.defineProperty(HTMLElement.prototype, "setPointerCapture", { value: setPointerCapture, configurable: true });
  render(<SpaceAgentTree />);
  const worktrees = screen.getAllByRole("treeitem").filter((item) => item.getAttribute("aria-level") === "1");
  const [target, source] = worktrees;
  const targetShell = target.parentElement!;
  vi.spyOn(targetShell, "getBoundingClientRect").mockReturnValue({
    top: 0, bottom: 100, left: 0, right: 200, width: 200, height: 100,
    x: 0, y: 0, toJSON: () => ({}),
  });
  const originalElementFromPoint = document.elementFromPoint;
  Object.defineProperty(document, "elementFromPoint", { value: vi.fn(() => null), configurable: true });
  fireEvent.pointerDown(source, { button: 0, pointerId: 9, clientX: 10, clientY: 10 });
  fireEvent.pointerMove(source, { pointerId: 9, clientX: 10, clientY: 30 });
  fireEvent.pointerUp(source, { pointerId: 9, clientX: 10, clientY: 30 });
  await vi.waitFor(() => expect(herdrWorkspaceMoveBlock).toHaveBeenCalledOnce());
  expect(setPointerCapture).toHaveBeenCalled();
  Object.defineProperty(document, "elementFromPoint", { value: originalElementFromPoint, configurable: true });
  Object.defineProperty(HTMLElement.prototype, "setPointerCapture", { value: originalSetPointerCapture, configurable: true });
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

it("Agents mode flattens only agent rows, preserves scoped identities and persists across remount", () => {
  const view = render(<SpaceAgentTree />);
  fireEvent.click(screen.getByRole("radio", { name: "Agents" }));
  expect(screen.getAllByRole("treeitem")).toHaveLength(3);
  expect(screen.getAllByRole("treeitem").every((row) => row.getAttribute("aria-level") === "1")).toBe(true);
  view.unmount();
  render(<SpaceAgentTree />);
  expect(screen.getByRole("radio", { name: "Agents" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getAllByRole("treeitem")).toHaveLength(3);
  fireEvent.click(screen.getByRole("radio", { name: "Spaces" }));
  expect(screen.getAllByRole("treeitem")).toHaveLength(9);
});
