import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import type { HerdrAttentionItem, HerdrSessionRuntime } from "@/lib/herdrTypes";
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore";
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces";
import i18n from "@/lib/i18n";
import { SpaceAgentTree } from "./SpaceAgentTree";

const scopes = ['["host-a","same"]', '["host-b","same"]', '["host-a","second"]'];
vi.mock("./HerdrLauncher", () => ({
  HerdrLauncher: ({ viewSwitcher, scope, onScopeChange }: {
    viewSwitcher: ReactNode; scope: string | null; onScopeChange: (name: string | null) => void;
  }) => <>{viewSwitcher}<select aria-label="Fixture Session scope" value={scope ?? ""}
    onChange={(event) => onScopeChange(event.target.value || null)}>
    <option value="">All</option>
    {scopes.map((name) => <option key={name} value={name}>{name}</option>)}
  </select></>,
}));

function item(sessionName: string, title: string, overrides: Partial<HerdrAttentionItem> = {}): HerdrAttentionItem {
  return { key: JSON.stringify([sessionName, title]), sessionName, paneId: "pane",
    workspaceId: "space", title, kind: "blocked", agentStatus: "blocked", seen: false,
    updatedAt: 1, ...overrides };
}
function seedAttention(items: HerdrAttentionItem[]) {
  const attention = new Map(items.map((entry) => [entry.key, entry]));
  useHerdrStore.setState({ attentionByKey: attention });
  return attention;
}
async function mountTree() {
  const view = render(<SpaceAgentTree />);
  await act(async () => {});
  return view;
}
function visibleTitles(container: HTMLElement) {
  return [...container.querySelectorAll(".space-tree-attention button")].map((row) => row.textContent?.split(" · ")[0]);
}
function expectCount(container: HTMLElement, count: number) {
  const section = container.querySelector(".space-tree-attention");
  if (count === 0) expect(section).toBeNull();
  else {
    expect(section?.querySelector("strong")).toHaveTextContent(`${i18n.t("spaceTree:attention")} · ${count}`);
    expect(section?.querySelectorAll("button")).toHaveLength(count);
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
  } });
  useRecentWorkspacesStore.setState({ presentations: {} });
  const runtimes: Record<string, HerdrSessionRuntime> = Object.fromEntries(scopes.map((scope) => [scope, {
    capabilities: null, connectionState: "ready", errorMessage: null, worktreeInventory: null,
    snapshot: { herdrSessionId: scope, protocol: 20, version: "0.8.2", raw: {}, tabs: [], terminals: [],
      spaces: [{ id: "space", label: "Project", path: "/fixture", order: 0, focused: true }],
      agents: [{ id: "agent", name: "Codex", status: "blocked", workspaceId: "space", paneId: "pane" }],
    },
  }]));
  useHerdrStore.setState({ ...herdrInitialState,
    sessions: scopes.map((scope, index) => ({ name: index === 2 ? "second" : "same", runtimeId: scope,
      hostId: index === 1 ? "host-b" : "host-a", hostLabel: index === 1 ? "B" : "A",
      running: true, default: false, sessionDir: "/fixture", socketPath: "/fixture/sock" })),
    selectedSessionName: scopes[0], selectedSpaceId: "space", runtimesBySession: runtimes,
    attentionByKey: new Map(), activateAgent: vi.fn().mockResolvedValue({ ok: true }), markAttentionSeen: vi.fn(),
  });
});
afterEach(cleanup);

it("filters stopped and removed Sessions in All without mutating their cached data", async () => {
  const attention = seedAttention(scopes.map((scope, index) => item(scope, `Attention ${index}`)));
  const runtimes = useHerdrStore.getState().runtimesBySession;
  useHerdrStore.setState({ sessions: useHerdrStore.getState().sessions.slice(0, 2).map((session, index) => ({ ...session, running: index === 0 })) });
  const { container } = await mountTree();
  expect(visibleTitles(container)).toEqual(["Attention 0"]);
  expectCount(container, 1);
  expect(useHerdrStore.getState().attentionByKey).toBe(attention);
  expect([...attention.values()].every((entry) => !entry.seen)).toBe(true);
  expect(useHerdrStore.getState().runtimesBySession).toBe(runtimes);
});

it.each(["stop", "remove"])("updates list and count on mounted %s and restores retained unread state on return", async (action) => {
  const attention = seedAttention([item(scopes[0], "A"), item(scopes[1], "B", { kind: "done", agentStatus: "done" })]);
  const sessions = useHerdrStore.getState().sessions;
  const { container } = await mountTree();
  expectCount(container, 2);
  act(() => useHerdrStore.setState({ sessions: action === "stop"
    ? sessions.map((session, index) => ({ ...session, running: index !== 1 }))
    : sessions.filter((_, index) => index !== 1) }));
  expect(visibleTitles(container)).toEqual(["A"]);
  expectCount(container, 1);
  expect(useHerdrStore.getState().attentionByKey).toBe(attention);
  act(() => useHerdrStore.setState({ sessions }));
  expect(visibleTitles(container)).toEqual(["A", "B"]);
  expectCount(container, 2);
  expect(useHerdrStore.getState().attentionByKey).toBe(attention);
  expect(useHerdrStore.getState().activateAgent).not.toHaveBeenCalled();
  expect(useHerdrStore.getState().markAttentionSeen).not.toHaveBeenCalled();
});

it.each(["stop", "remove"])("falls back from selected Session to All on %s without reviving its Attention", async (action) => {
  seedAttention(scopes.map((scope, index) => item(scope, `Attention ${index}`)));
  const sessions = useHerdrStore.getState().sessions;
  const { container } = await mountTree();
  fireEvent.change(screen.getByRole("combobox", { name: "Fixture Session scope" }), { target: { value: scopes[1] } });
  expect(visibleTitles(container)).toEqual(["Attention 1"]);
  expectCount(container, 1);
  act(() => useHerdrStore.setState({ sessions: action === "stop"
    ? sessions.map((session, index) => ({ ...session, running: index !== 1 }))
    : sessions.filter((_, index) => index !== 1) }));
  expect(screen.getByRole("combobox")).toHaveValue("");
  expect(visibleTitles(container)).toEqual(["Attention 0", "Attention 2"]);
  expectCount(container, 2);
});

it.each(["connecting", "error"] as const)("retains Attention for a running %s Session with cached topology", async (connectionState) => {
  seedAttention([item(scopes[0], "A")]);
  const runtimes = useHerdrStore.getState().runtimesBySession;
  useHerdrStore.setState({ runtimesBySession: { ...runtimes, [scopes[0]]: { ...runtimes[scopes[0]], connectionState } } });
  const { container } = await mountTree();
  expect(visibleTitles(container)).toEqual(["A"]);
  expectCount(container, 1);
});

it("keeps a running Session event that arrives before its topology snapshot", async () => {
  seedAttention([item(scopes[0], "Early event", { paneId: "new-pane" })]);
  const { container } = await mountTree();
  expect(screen.getByRole("button", { name: /Early event/ })).toBeDisabled();
  expectCount(container, 1);
});

it.each(["ready", "failed"] as const)("keeps only startup-pending local default Attention until startup is %s", async (state) => {
  const local = { name: "default", default: true, running: false, sessionDir: "/fixture", socketPath: "/fixture/sock" };
  const remote = '["remote","default"]';
  const attention = seedAttention([item("default", "Local"), item("named", "Named"), item(remote, "Remote")]);
  useHerdrStore.setState({ herdrStartup: { state: "starting", error: null }, sessions: [local,
    { ...local, name: "named", default: false }, { ...local, hostId: "remote", runtimeId: remote }],
    selectedSessionName: "default", runtimesBySession: {} });
  const { container } = await mountTree();
  expect(visibleTitles(container)).toEqual(["Local"]);
  expectCount(container, 1);
  act(() => useHerdrStore.setState({ herdrStartup: { state, error: state === "failed" ? "failed" : null } }));
  expectCount(container, 0);
  expect(useHerdrStore.getState().attentionByKey).toBe(attention);
});

it("preserves kind, seen and sorting rules through Session return and Agents view", async () => {
  const attention = seedAttention([
    item(scopes[0], "Seen blocked", { seen: true, updatedAt: 1 }),
    item(scopes[0], "New blocked", { updatedAt: 2 }),
    item(scopes[0], "Seen done", { kind: "done", agentStatus: "done", seen: true, updatedAt: 100 }),
    item(scopes[0], "Unread done", { kind: "done", agentStatus: "done", updatedAt: 3 }),
    item(scopes[0], "Unknown", { kind: "unknown", agentStatus: "unknown", seen: true, updatedAt: 4 }),
    item(scopes[0], "Error", { kind: "error", agentStatus: "unknown", updatedAt: 5 }),
  ]);
  const sessions = useHerdrStore.getState().sessions;
  const { container } = await mountTree();
  const expected = ["New blocked", "Seen blocked", "Error", "Unknown", "Unread done"];
  expect(visibleTitles(container)).toEqual(expected);
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Agents" }), { button: 0, ctrlKey: false });
  expect(visibleTitles(container)).toEqual(expected);
  act(() => useHerdrStore.setState({ sessions: [] }));
  expectCount(container, 0);
  act(() => useHerdrStore.setState({ sessions }));
  expect(visibleTitles(container)).toEqual(expected);
  expectCount(container, 5);
  expect(within(container).queryByRole("button", { name: /^Seen done/ })).not.toBeInTheDocument();
  expect(useHerdrStore.getState().attentionByKey).toBe(attention);
});
