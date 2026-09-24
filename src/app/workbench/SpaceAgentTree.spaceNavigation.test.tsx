import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { SpaceAgentTree } from "./SpaceAgentTree";
import "@/lib/i18n";
import { useHerdrStore, herdrInitialState } from "@/state/herdrStore";
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces";
import { rememberedSpaceBranch, resetSpaceBranchMemoryCache } from "@/lib/spaceBranchMemory";
import type { HerdrSessionRuntime, HerdrSnapshot, HerdrSpaceInfo } from "@/lib/herdrTypes";

vi.mock("./HerdrLauncher", () => ({ HerdrLauncher: () => null }));

const sessions = ["s1", "s2"];
const space = (id: string, group: string, branch: string, linked: boolean, order: number): HerdrSpaceInfo => ({
  id, label: `${group}-${branch}`, path: `/${group}/${branch}`, order, focused: false,
  branch, worktreeGroupKey: group, repoRoot: `/${group}`, isLinkedWorktree: linked,
} as HerdrSpaceInfo);

let activateSpace: ReturnType<typeof vi.fn>;
beforeEach(() => {
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) },
  });
  resetSpaceBranchMemoryCache();
  useRecentWorkspacesStore.setState({ presentations: {} });
  const runtimes: Record<string, HerdrSessionRuntime> = {};
  for (const name of sessions) {
    runtimes[name] = {
      capabilities: { server: { running: true }, api: { workspaceFocus: true } } as HerdrSessionRuntime["capabilities"],
      connectionState: "ready",
      errorMessage: null,
      worktreeInventory: null,
      snapshot: {
        herdrSessionId: name, protocol: 22, version: "0.9.1",
        spaces: [space("a", "repo", "main", false, 0), space("b", "repo", "feature", true, 1), space("q", "other", "main", false, 2)],
        agents: [], tabs: [], terminals: [], raw: {},
      } as HerdrSnapshot,
    };
  }
  activateSpace = vi.fn(async ({ sessionName, workspaceId }: { sessionName: string; workspaceId: string }) => {
    useHerdrStore.setState({ selectedSessionName: sessionName, selectedSpaceId: workspaceId });
    return { ok: true };
  });
  useHerdrStore.setState({
    ...herdrInitialState,
    activateSpace: activateSpace as never,
    sessions: sessions.map((name) => ({ name, running: true, default: name === "s1", sessionDir: "/", socketPath: "/sock" })),
    runtimesBySession: runtimes,
    selectedSessionName: "s1",
    selectedSpaceId: "q",
    attentionByKey: new Map(),
  });
});
afterEach(cleanup);

const spaceRows = () => screen.getAllByRole("treeitem").filter((row) => row.getAttribute("aria-level") === "1");
const branchRows = () => screen.getAllByRole("treeitem").filter((row) => row.getAttribute("aria-level") === "2");
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });

it("opens the first branch of a Space that has no remembered branch and expands it", async () => {
  render(<SpaceAgentTree />);
  fireEvent.click(spaceRows()[0]);
  expect(activateSpace).toHaveBeenCalledWith({ sessionName: "s1", workspaceId: "a", path: "/repo/main" });
  expect(spaceRows()[0]).toHaveAttribute("aria-expanded", "true");
});

it("returns to the remembered branch, including after a remount", async () => {
  const view = render(<SpaceAgentTree />);
  act(() => useHerdrStore.setState({ selectedSpaceId: "b" }));
  await flush();
  act(() => useHerdrStore.setState({ selectedSpaceId: "q" }));
  await flush();
  view.unmount();
  resetSpaceBranchMemoryCache();
  render(<SpaceAgentTree />);
  fireEvent.click(spaceRows()[0]);
  expect(activateSpace).toHaveBeenLastCalledWith({ sessionName: "s1", workspaceId: "b", path: "/repo/feature" });
});

it("falls back to the first branch when the remembered one no longer exists", async () => {
  const view = render(<SpaceAgentTree />);
  act(() => useHerdrStore.setState({ selectedSpaceId: "b" }));
  await flush();
  act(() => useHerdrStore.setState({ selectedSpaceId: "q" }));
  await flush();
  view.unmount();
  const runtime = useHerdrStore.getState().runtimesBySession.s1;
  useHerdrStore.setState({
    runtimesBySession: {
      ...useHerdrStore.getState().runtimesBySession,
      s1: { ...runtime, snapshot: { ...runtime.snapshot!, spaces: runtime.snapshot!.spaces.filter((item) => item.id !== "b") } },
    },
  });
  render(<SpaceAgentTree />);
  fireEvent.click(spaceRows()[0]);
  expect(activateSpace).toHaveBeenLastCalledWith({ sessionName: "s1", workspaceId: "a", path: "/repo/main" });
});

it("keeps the Space that already contains the selection as it is", async () => {
  render(<SpaceAgentTree />);
  const other = spaceRows()[1];
  expect(other).toHaveAttribute("aria-expanded", "true");
  fireEvent.click(other);
  expect(activateSpace).not.toHaveBeenCalled();
  expect(spaceRows()[1]).toHaveAttribute("aria-expanded", "true");
});

it("toggles a Space from its in-row chevron without switching branches", () => {
  render(<SpaceAgentTree />);
  const before = branchRows().length;
  const toggle = screen.getAllByRole("button", { name: /^Collapse repo/ })[0];
  expect(toggle.closest(".tree-row-actions")).not.toBeNull();
  fireEvent.click(toggle);
  expect(activateSpace).not.toHaveBeenCalled();
  expect(branchRows().length).toBe(before - 2);
  fireEvent.click(screen.getAllByRole("button", { name: /^Expand repo/ })[0]);
  expect(branchRows().length).toBe(before);
});

it("keeps branch memory separate for the same group in different Sessions", async () => {
  render(<SpaceAgentTree />);
  act(() => useHerdrStore.setState({ selectedSpaceId: "b" }));
  await flush();
  const s1Key = JSON.stringify(["s1", "project", "repo"]);
  const s2Key = JSON.stringify(["s2", "project", "repo"]);
  expect(rememberedSpaceBranch(s1Key)).toBe("b");
  expect(rememberedSpaceBranch(s2Key)).toBeNull();
  fireEvent.click(spaceRows()[2]);
  expect(activateSpace).toHaveBeenLastCalledWith({ sessionName: "s2", workspaceId: "a", path: "/repo/main" });
});

it("places branch and Space actions inside the row and keeps them keyboard reachable", () => {
  render(<SpaceAgentTree />);
  const branch = branchRows()[0];
  const shell = branch.closest(".tree-row-shell") as HTMLElement;
  fireEvent.focus(branch);
  const actions = within(shell).getAllByRole("button").filter((button) => button !== branch);
  expect(actions.length).toBeGreaterThan(0);
  for (const action of actions) {
    expect(action.closest(".tree-row-actions")).not.toBeNull();
    expect(action).toHaveAttribute("tabindex", "0");
  }
});
