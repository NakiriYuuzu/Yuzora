import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import "@/lib/i18n";
import { SpaceAgentTree } from "./SpaceAgentTree";
import { useHerdrStore, herdrInitialState } from "@/state/herdrStore";
import { useUiStore } from "@/state/uiStore";
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces";
import { useAgentMruStore, agentMruKey } from "@/state/agentMruStore";
import { useKeyboardSettingsStore } from "@/state/keyboardSettingsStore";
import type { HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes";

vi.mock("./HerdrLauncher", () => ({
  HerdrLauncher: ({ viewSwitcher }: { viewSwitcher: import("react").ReactNode }) => viewSwitcher,
}));

let storage: Map<string, string>;
let activateAgent: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows");
  storage = new Map();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) },
  });
  useRecentWorkspacesStore.setState({ presentations: {} });
  useKeyboardSettingsStore.setState({ overrides: {} });
  useAgentMruStore.setState({ keys: [] });
  activateAgent = vi.fn().mockResolvedValue({ ok: true });
  const agents = ["one", "two", "three"].map((id) => ({
    id, name: id === "one" ? "pi" : "codex", title: `Agent ${id}`, status: "idle", workspaceId: "space", paneId: `p-${id}`, terminalId: `t-${id}`,
  }));
  useHerdrStore.setState({
    ...herdrInitialState,
    activateAgent: activateAgent as never,
    sessions: [{ name: "s1", running: true, default: true, sessionDir: "/", socketPath: "/sock" }],
    runtimesBySession: {
      s1: {
        connectionState: "ready", errorMessage: null, worktreeInventory: null,
        capabilities: { server: { running: true }, api: { workspaceFocus: true, tabFocus: true } } as HerdrSessionRuntime["capabilities"],
        snapshot: {
          herdrSessionId: "s1", protocol: 22, version: "0.9.1",
          spaces: [{ id: "space", label: "Project", path: "/repo", order: 0, focused: true }],
          agents, tabs: [], terminals: [], raw: {},
        } as HerdrSnapshot,
      },
    },
    selectedSessionName: "s1",
    selectedSpaceId: "space",
    attentionByKey: new Map(),
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("switches Spaces/Agents from the uiStore request and persists the choice", () => {
  render(<SpaceAgentTree />);
  expect(screen.getByRole("tab", { name: "Spaces" })).toHaveAttribute("aria-selected", "true");
  act(() => useUiStore.getState().requestSidebarViewToggle());
  expect(screen.getByRole("tab", { name: "Agents" })).toHaveAttribute("aria-selected", "true");
  expect(storage.get("yuzora.sidebar.view")).toBe("agents");
  act(() => useUiStore.getState().requestSidebarViewToggle());
  expect(screen.getByRole("tab", { name: "Spaces" })).toHaveAttribute("aria-selected", "true");
});

it("labels the first nine Agents with shortcuts and shows badges while Alt is held", () => {
  storage.set("yuzora.sidebar.view", "agents");
  render(<SpaceAgentTree />);
  const rows = screen.getAllByRole("treeitem");
  expect(rows.map((row) => row.getAttribute("aria-keyshortcuts"))).toEqual(["Alt+1", "Alt+2", "Alt+3"]);
  expect(document.querySelector(".tree-agent-shortcut")).toBeNull();
  fireEvent.keyDown(window, { key: "Alt", code: "AltLeft", altKey: true });
  expect([...document.querySelectorAll(".tree-agent-shortcut")].map((badge) => badge.textContent)).toEqual(["Alt+1", "Alt+2", "Alt+3"]);
  fireEvent.keyUp(window, { key: "Alt", code: "AltLeft" });
  expect(document.querySelector(".tree-agent-shortcut")).toBeNull();
});

it("jumps with Alt+N and cycles with Alt+` without moving focus into the sidebar", () => {
  render(<SpaceAgentTree />);
  const terminal = document.body.appendChild(document.createElement("textarea"));
  terminal.focus();
  fireEvent.keyDown(terminal, { key: "3", code: "Digit3", altKey: true });
  expect(activateAgent).toHaveBeenLastCalledWith(expect.objectContaining({ id: "three", sessionName: "s1" }));
  expect(document.activeElement).toBe(terminal);
  act(() => useAgentMruStore.setState({ keys: [agentMruKey("s1", "three")] }));
  fireEvent.keyDown(terminal, { key: "`", code: "Backquote", altKey: true });
  const options = screen.getAllByRole("option");
  expect(options.map((option) => option.textContent)).toEqual([
    expect.stringContaining("Agent three"), expect.stringContaining("Agent one"), expect.stringContaining("Agent two"),
  ]);
  expect(options[1]).toHaveAttribute("aria-selected", "true");
  fireEvent.keyUp(terminal, { key: "Alt", code: "AltLeft" });
  expect(activateAgent).toHaveBeenLastCalledWith(expect.objectContaining({ id: "one" }));
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(document.activeElement).toBe(terminal);
  terminal.remove();
});
