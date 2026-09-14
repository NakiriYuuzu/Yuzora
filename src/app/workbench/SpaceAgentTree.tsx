import { contextMenuHandler } from "@/state/contextMenuStore";
import {
  Fragment,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type MouseEvent,
} from "react";
import {
  ChevronsDownUp,
  ChevronsUpDown,
  GitBranch,
  EllipsisVertical,
  Info,
  Plus,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { ScrollArea } from "@/components/ui/scroll-area";
import { HerdrAgentInspector } from "@/app/workbench/HerdrAgentInspector";
import { resolveProjectPresentation } from "@/app/workbench/projectPresentation";
import { useHerdrStore } from "@/state/herdrStore";
import { useUiStore } from "@/state/uiStore";
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces";

import { sortHerdrAgentsByUrgency } from "@/lib/herdrAgents";
import type { HerdrAgentInfo, HerdrSpaceInfo } from "@/lib/herdrTypes";
import { parseRuntimeScope, runtimeOwner, sessionScope } from "@/lib/herdrProvider";
import { spacePresentationKey, runtimeSessionLabel } from "./spaceTreeIdentity";
import { chooseWorkspaceFolder } from "@/state/folderPickerStore";
import { workspacePathBasename } from "@/lib/paths";
import { openCreatedHerdrTabAndRequestName } from "@/lib/herdrTabActions";
import { canMoveHerdrWorkspace, moveHerdrWorkspace } from "@/lib/herdrWorkspaceActions";
import { HerdrLauncher } from "./HerdrLauncher";
import { SpaceAppearanceDialog } from "./SpaceAppearanceDialog";
import { SpaceCharacter } from "./SpaceCharacter";
import type { SpaceCharacterConfig } from "./space-character";

interface TreeNode {
  key: string;
  sessionName: string;
  parent?: string;
  level: number;
  position: number;
  size: number;
  kind: "project" | "worktree" | "agent";
  label: string;
  space: HerdrSpaceInfo;
  identityKey?: string;
  agent?: HerdrAgentInfo;
  children: TreeNode[];
  count: number;
  pending: number;
  color?: { background: string; foreground: string };
  glyph?: string;
  avatarMode?: "character" | "glyph";
  character?: SpaceCharacterConfig;
}

/** Runtime-backed navigation chrome. Project folders are presentation groups, never runtime IDs.
 * Flat ARIA treeitems carry explicit levels and sibling positions; shadcn owns primitives.
 */
export function SpaceAgentTree() {
  const { t } = useTranslation("spaceTree");
  const [viewMode, setViewMode] = useState<"spaces" | "agents">(() => {
    try { return localStorage.getItem("yuzora.sidebar.view") === "agents" ? "agents" : "spaces"; }
    catch { return "spaces"; }
  });
  const session = useHerdrStore((s) => s.selectedSessionName) ?? "";
  const rawSessions = useHerdrStore((s) => s.sessions),
    runtimes = useHerdrStore((s) => s.runtimesBySession);
  const sessions = useMemo(
    () => rawSessions.filter((item) => item.running).map((item) => ({ ...item, name: sessionScope(item)! })),
    [rawSessions],
  );
  const attention = useHerdrStore((s) => s.attentionByKey);
  const selectedSpace = useHerdrStore((s) => s.selectedSpaceId);
  const presentations = useRecentWorkspacesStore((s) => s.presentations);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<string | null>(null);
  const [inspected, setInspected] = useState<HerdrAgentInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reorderError, setReorderError] = useState<string | null>(null);
  const [creatingSpace, setCreatingSpace] = useState(false);
  const [creatingTerminal, setCreatingTerminal] = useState<string | null>(null);
  const terminalCreationInFlight = useRef(false);
  const [draggedSpaceKey, setDraggedSpaceKey] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<{ key: string; after: boolean } | null>(null);
  const dropTargetRef = useRef<{ key: string; after: boolean } | null>(null);
  const [movingSpaceKey, setMovingSpaceKey] = useState<string | null>(null);
  const draggedSpaceRef = useRef<{ key: string; sessionName: string; workspaceId: string } | null>(null);
  const gestureCleanup = useRef<(() => void) | null>(null);
  const movingRef = useRef(false);
  const treeContainer = useRef<HTMLDivElement>(null);
  useEffect(() => () => gestureCleanup.current?.(), []);
  const suppressClickRef = useRef(false);
  // All is a view filter, never a runtime Session name or process context.
  const [requestedScope, setScopeSession] = useState<string | null>(null);
  const scopeSession = sessions.some((item) => item.name === requestedScope) ? requestedScope : null;
  const [editingSpace, setEditingSpace] = useState<TreeNode | null>(null);
  const editTrigger = useRef<HTMLButtonElement | null>(null);
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const inspectorTrigger = useRef<HTMLButtonElement | null>(null);
  const previousOwner = useRef("");
  const shownSessions = sessions.filter(
    (item) => scopeSession === null || item.name === scopeSession,
  );
  function repairHost(name?: string) {
    useUiStore.getState().openSettings("herdr", { hostId: name ? parseRuntimeScope(name).hostId : undefined });
  }
  function needsRepair(name: string) {
    const runtime = runtimes[name];
    return runtime?.capabilities?.server.compatible === false || runtime?.connectionState === "unsupported" || !!runtime?.errorMessage;
  }
  function sessionNotice(name: string) {
    const runtime = runtimes[name];
    const caps = runtime?.capabilities;
    if (caps?.server.compatible === false) {
      return `${t("incompatibleDetail", {
        clientVersion: caps.binaryVersion ?? "—",
        clientProtocol: caps.binaryProtocol ?? "—",
        serverVersion: caps.server.version ?? "—",
        serverProtocol: caps.server.protocol ?? "—",
      })} ${t("binaryDetail", { path: caps.binaryPath ?? "—" })} ${t("incompatibleRecovery")}`;
    }
    if (runtime?.connectionState === "unsupported")
      return runtime.errorMessage ?? t("stale");
    if (
      runtime?.connectionState === "stopped" ||
      runtime?.capabilities?.server.running === false
    )
      return t("stopped");
    if (runtime?.errorMessage)
      return runtime.snapshot
        ? `${t("stale")} ${runtime.errorMessage}`
        : runtime.errorMessage;
    if (!runtime || runtime.connectionState === "idle" || runtime.connectionState === "connecting")
      return t("loading");
    if (runtime?.connectionState !== "ready" || !runtime?.snapshot)
      return t("stale");
    return null;
  }
  function sessionLabel(name: string) {
    const named = rawSessions.find((item) => sessionScope(item) === name);
    return runtimeSessionLabel(name, named);
  }
  function canAddTerminal(name: string) {
    const runtime = runtimes[name];
    const caps = runtime?.capabilities;
    return runtime?.connectionState === "ready" && !runtime.errorMessage &&
      caps?.server.compatible !== false && !!caps?.server.running &&
      !!caps.api.snapshot && !!caps.api.workspaceFocus &&
      !!caps.api.tabCreate && !!caps.terminal.create;
  }

  function canReorderSpace(node: TreeNode) {
    return node.kind === "project" && canMoveHerdrWorkspace(node.sessionName, node.space.id);
  }

  function clearSpaceDrag() {
    draggedSpaceRef.current = null;
    dropTargetRef.current = null;
    setDraggedSpaceKey(null);
    setDropTarget(null);
  }

  function updatePointerDropTarget(clientX: number, clientY: number) {
    const source = draggedSpaceRef.current, container = treeContainer.current;
    if (!source || !container) return null;
    const viewport = container.querySelector<HTMLElement>('[data-slot="scroll-area-viewport"]');
    const clip = viewport?.getBoundingClientRect();
    const hit = document.elementFromPoint?.(clientX, clientY);
    const inside = !clip || (clientX >= clip.left && clientX <= clip.right && clientY >= clip.top && clientY <= clip.bottom);
    // Actual occlusion beats geometric fallback. A null hit may occur during capture.
    const hitScope = hit?.closest<HTMLElement>("[data-session-scope]")?.dataset.sessionScope;
    const occluded = hit && (!container.contains(hit) || (hitScope !== undefined && hitScope !== source.sessionName));
    let next: { key: string; after: boolean } | null = null;
    if (inside && !occluded) {
      const rows = [...container.querySelectorAll<HTMLElement>('[data-space-key]')];
      let distance = Infinity;
      for (const row of rows) {
        const node = all.find(item => item.key === row.dataset.spaceKey);
        if (!node || node.sessionName !== source.sessionName || !canReorderSpace(node)) continue;
        const rect = row.getBoundingClientRect();
        if (!rect.height || !rect.width || (clip && (rect.bottom <= clip.top || rect.top >= clip.bottom))) continue;
        if (clientX < rect.left || clientX > rect.right) continue;
        const after = clientY > rect.top + rect.height / 2;
        const delta = Math.abs(clientY - (after ? rect.bottom : rect.top));
        if (delta < distance) { distance = delta; next = { key: node.key, after }; }
      }
    }
    dropTargetRef.current = next;
    setDropTarget(current => current?.key === next?.key && current?.after === next?.after ? current : next);
    return next;
  }

  function onSpacePointerDown(event: PointerEvent<HTMLElement> | MouseEvent<HTMLElement>, node: TreeNode) {
    if (!canReorderSpace(node) || event.button !== 0 || gestureCleanup.current || movingRef.current) return;
    const pointerId = "pointerId" in event ? event.pointerId : null;
    const source = { key: node.key, sessionName: node.sessionName, workspaceId: node.space.id, owner: JSON.stringify(runtimeOwner(node.sessionName)) };
    const startX = event.clientX, startY = event.clientY, element = event.currentTarget;
    let active = false;
    suppressClickRef.current = false;
    const moveName = pointerId === null ? "mousemove" : "pointermove";
    const upName = pointerId === null ? "mouseup" : "pointerup";
    const matches = (e: Event) => pointerId === null || (e as globalThis.PointerEvent).pointerId === pointerId;
    const cleanup = () => {
      window.removeEventListener(moveName, move, true);
      window.removeEventListener(upName, up, true);
      window.removeEventListener("pointercancel", cancel, true);
      window.removeEventListener("blur", cancel);
      window.removeEventListener("keydown", key, true);
      gestureCleanup.current = null;
      if (pointerId !== null && element.hasPointerCapture?.(pointerId)) element.releasePointerCapture(pointerId);
    };
    const cancel = (e?: Event) => {
      if (e?.type === "pointercancel" && !matches(e)) return;
      cleanup(); clearSpaceDrag();
    };
    const key = (e: KeyboardEvent | globalThis.KeyboardEvent) => { if (e.key === "Escape") cancel(); };
    const move = (e: Event) => {
      if (!matches(e)) return;
      const point = e as globalThis.MouseEvent;
      if (!active && Math.hypot(point.clientX - startX, point.clientY - startY) < 5) return;
      if (!active) {
        active = true; suppressClickRef.current = true;
        draggedSpaceRef.current = source; setDraggedSpaceKey(source.key);
        try { if (pointerId !== null) element.setPointerCapture?.(pointerId); } catch { /* Global listeners retain this gesture. */ }
      }
      e.preventDefault(); updatePointerDropTarget(point.clientX, point.clientY);
    };
    const up = (e: Event) => {
      if (!matches(e)) return;
      const point = e as globalThis.MouseEvent;
      const target = active ? updatePointerDropTarget(point.clientX, point.clientY) : null;
      cleanup(); clearSpaceDrag();
      // Suppress only the click synthesized by this drop, not a later click
      // on a child row (which does not start a project drag gesture).
      if (active) window.setTimeout(() => { suppressClickRef.current = false; }, 0);
      if (target) void moveSpace(source, target);
    };
    window.addEventListener(moveName, move, true);
    window.addEventListener(upName, up, true);
    window.addEventListener("pointercancel", cancel, true);
    window.addEventListener("blur", cancel);
    window.addEventListener("keydown", key, true);
    gestureCleanup.current = cleanup;
  }

  async function moveSpace(source: { key: string; sessionName: string; workspaceId: string; owner: string }, target: { key: string; after: boolean }) {
    if (source.owner !== JSON.stringify(runtimeOwner(source.sessionName))) return;
    const node = all.find(item => item.key === target.key);
    if (!node || source.sessionName !== node.sessionName || !canReorderSpace(node) || movingRef.current) return;
    movingRef.current = true; setMovingSpaceKey(source.key); setError(null); setReorderError(null);
    try {
      await moveHerdrWorkspace(source.sessionName, source.workspaceId, node.space.id, target.after);
    } catch (cause) {
      setReorderError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      movingRef.current = false; setMovingSpaceKey(null);
    }
  }
  async function addTerminal(node: TreeNode) {
    if (terminalCreationInFlight.current || creatingSpace || !canAddTerminal(node.sessionName)) return;
    terminalCreationInFlight.current = true;
    setCreatingTerminal(node.key);
    setError(null);
    focus(node.key);
    try {
      const activated = await useHerdrStore.getState().activateSpace({
        sessionName: node.sessionName,
        workspaceId: node.space.id,
        path: node.space.path,
      });
      if (!activated.ok) {
        if (!activated.cancelled) setError(activated.error ?? t("switchFailed"));
        return;
      }
      // Activation may await an unsaved-work prompt or a remote host. Recheck
      // the owner before invoking the existing selected-Space creation action.
      const state = useHerdrStore.getState();
      if (state.selectedSessionName !== node.sessionName || state.selectedSpaceId !== node.space.id)
        throw new Error(t("terminalTargetChanged"));
      if (state.connectionState !== "ready" || state.errorMessage ||
          state.capabilities?.server.compatible === false || !state.canCreateTerminal())
        throw new Error(state.errorMessage ?? state.createTerminalBlockedReason() ?? t("terminalUnavailable"));
      const created = await state.createTerminalInSelectedSpace();
      if (!created)
        throw new Error(useHerdrStore.getState().runtimesBySession[node.sessionName]?.errorMessage ??
          t("herdrNav.createFailedUnknown", { ns: "workbench" }));
      setSelectedAgent(null);
      expand(node.key, true);
      await openCreatedHerdrTabAndRequestName({
        sessionName: created.herdrSessionId,
        workspaceId: created.workspaceId,
        terminalId: created.terminalId,
        title: created.title,
        paneId: created.paneId,
        tabId: created.tabId,
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      terminalCreationInFlight.current = false;
      setCreatingTerminal(null);
    }
  }
  async function createSpace(sessionName: string) {
    if (creatingSpace || terminalCreationInFlight.current) return;
    setCreatingSpace(true);
    setError(null);
    try {
      if (useHerdrStore.getState().selectedSessionName !== sessionName)
        await useHerdrStore.getState().selectSession(sessionName);
      const state = useHerdrStore.getState();
      if (state.selectedSessionName !== sessionName || !state.canCreateSpace())
        throw new Error(state.createSpaceBlockedReason() ?? t("openFailed"));
      const path = await chooseWorkspaceFolder({ runtimeHostId: parseRuntimeScope(sessionName).hostId });
      if (typeof path !== "string") return;
      // The folder belongs to the requested host, even if selection changed while the picker was open.
      if (useHerdrStore.getState().selectedSessionName !== sessionName)
        throw new Error(t("sessionChanged"));
      const result = await useHerdrStore.getState().createSpaceFromFolder(path, workspacePathBasename(path));
      if (!result.ok && !result.cancelled)
        throw new Error(result.error ?? t("openFailed"));
      if (result.ok) setScopeSession(sessionName);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setCreatingSpace(false);
    }
  }

  const roots = useMemo(
    () =>
      sessions
        .filter((item) => scopeSession === null || item.name === scopeSession)
        .flatMap(({ name: sessionName }) => {
          const snapshot = runtimes[sessionName]?.snapshot;
          const groups = new Map<string, HerdrSpaceInfo[]>();
          for (const space of snapshot?.spaces ?? []) {
            const key = space.worktreeGroupKey ?? space.repoKey ?? space.id;
            groups.set(key, [...(groups.get(key) ?? []), space]);
          }
          return [...groups.entries()].map(
            ([group, spaces], index): TreeNode => {
              const first = spaces.find(space => space.isLinkedWorktree === false) ?? spaces[0],
                root = first.repoRoot ?? first.path ?? first.label;
              const identityKey = spacePresentationKey(sessionName, root);
              const identity = resolveProjectPresentation(
                root,
                presentations[identityKey],
              );
              const key = JSON.stringify([sessionName, "project", group]);
              const children = spaces.map((space, position): TreeNode => {
                const branchKey = JSON.stringify([
                  sessionName,
                  "worktree",
                  space.id,
                ]);
                const unseenDone = (agent: HerdrAgentInfo) =>
                  agent.status === "done" &&
                  [...attention.values()].some(
                    (item) =>
                      item.sessionName === sessionName &&
                      item.paneId === agent.paneId &&
                      item.kind === "done" &&
                      !item.seen,
                  );
                const priority = (agent: HerdrAgentInfo) =>
                  agent.status === "blocked" ? 0 : unseenDone(agent) ? 1 : 2;
                const agents = sortHerdrAgentsByUrgency(
                  (snapshot?.agents ?? []).filter(
                    (agent) => agent.workspaceId === space.id,
                  ),
                ).sort((a, b) => priority(a) - priority(b));
                return {
                  key: branchKey,
                  sessionName,
                  parent: key,
                  level: 2,
                  position: position + 1,
                  size: spaces.length,
                  kind: "worktree",
                  label: space.branch ?? space.label,
                  space,
                  count: agents.length,
                  pending: agents.filter(
                    (agent) => agent.status === "blocked" || unseenDone(agent),
                  ).length,
                  children: agents.map((agent, agentIndex) => ({
                    key: JSON.stringify([sessionName, "agent", agent.id]),
                    sessionName,
                    parent: branchKey,
                    level: 3,
                    position: agentIndex + 1,
                    size: agents.length,
                    kind: "agent",
                    label: agent.title ?? agent.name ?? agent.id,
                    space,
                    agent: { ...agent, sessionName },
                    children: [],
                    count: 0,
                    pending: 0,
                  })),
                };
              });
              return {
                key,
                sessionName,
                level: 1,
                position: index + 1,
                size: groups.size,
                kind: "project",
                label: identity.name,
                space: first,
                identityKey,
                color: identity.color,
                glyph: identity.glyph,
                avatarMode: identity.avatarMode,
                character: identity.character,
                children,
                count: children.reduce((n, child) => n + child.count, 0),
                pending: children.reduce((n, child) => n + child.pending, 0),
              };
            },
          );
        })
        .map((root, index, items) => ({
          ...root,
          position: index + 1,
          size: items.length,
        })),
    [runtimes, presentations, sessions, scopeSession, attention],
  );
  // Persist once before the first paint, including Spaces discovered from HERDR.
  useLayoutEffect(() => {
    useRecentWorkspacesStore.getState().ensureSpacePresentations(
      roots.flatMap((root) => root.identityKey ? [root.identityKey] : []),
    );
  }, [roots]);
  const all = roots.flatMap((root) => [
    root,
    ...root.children.flatMap((branch) => [branch, ...branch.children]),
  ]);
  const visible: TreeNode[] = viewMode === "agents"
    ? all.filter((node) => node.kind === "agent").map((node, index, nodes) => ({ ...node, parent: undefined, level: 1, position: index + 1, size: nodes.length }))
    : roots.flatMap((root) => [
    root,
    ...(collapsed.has(root.key)
      ? []
      : root.children.flatMap((branch) => [
          branch,
          ...(collapsed.has(branch.key) ? [] : branch.children),
        ])),
  ]);
  const selectedLeaf = all.find(
    (node) =>
      node.key === selectedAgent &&
      node.sessionName === session &&
      node.space.id === selectedSpace,
  );
  const selectedKey =
    selectedLeaf?.key ??
    all.find(
      (node) =>
        node.kind === "worktree" &&
        node.sessionName === session &&
        node.space.id === selectedSpace,
    )?.key;
  const tabKey = visible.some((node) => node.key === focusKey)
    ? focusKey
    : (visible.find((node) => node.key === selectedKey)?.key ??
      visible[0]?.key);

  // Breadcrumb/Session changes reveal their owning folder, without reopening a
  // folder the user deliberately collapses while staying on the same checkout.
  useEffect(() => {
    const owner = JSON.stringify([session, selectedSpace]);
    if (previousOwner.current === owner) return;
    let active = true;
    queueMicrotask(() => {
      if (!active) return;
      previousOwner.current = owner;
      if (scopeSession !== null && scopeSession !== session)
        setScopeSession(session);
      const branch = roots
        .flatMap((root) => root.children)
        .find(
          (node) =>
            node.sessionName === session && node.space.id === selectedSpace,
        );
      if (branch)
        setCollapsed((current) => {
          const next = new Set(current);
          next.delete(branch.key);
          if (branch.parent) next.delete(branch.parent);
          return next;
        });
      setError(null);
    });
    return () => {
      active = false;
    };
  }, [session, selectedSpace, roots, scopeSession]);

  function focus(key: string) {
    setFocusKey(key);
    refs.current.get(key)?.focus();
    refs.current.get(key)?.scrollIntoView?.({ block: "nearest" });
  }
  function expand(key: string, open: boolean) {
    setCollapsed((current) => {
      const next = new Set(current);
      if (open) next.delete(key);
      else next.add(key);
      return next;
    });
  }
  function activate(node: TreeNode) {
    focus(node.key);
    if (node.kind === "project") {
      expand(node.key, collapsed.has(node.key));
      return;
    }
    if (node.kind === "agent") {
      setSelectedAgent(node.key);
      const runtime = runtimes[node.sessionName],
        caps = runtime?.capabilities;
      if (
        runtime?.connectionState === "ready" &&
        caps?.server.running &&
        caps.api.workspaceFocus &&
        node.agent?.terminalId &&
        (!node.agent.tabId || caps.api.tabFocus)
      ) {
        void useHerdrStore
          .getState()
          .activateAgent(node.agent)
          .then((result) => {
            if (!result.ok && !result.cancelled)
              setError(result.error ?? t("switchFailed"));
          })
          .catch((error) => setError(String(error)));
      } else {
        inspectorTrigger.current = refs.current.get(node.key) ?? null;
        setInspected(node.agent!);
      }
    } else {
      const runtime = runtimes[node.sessionName];
      if (
        runtime?.connectionState !== "ready" ||
        !runtime.capabilities?.server.running ||
        !runtime.capabilities.api.workspaceFocus
      ) {
        setError(sessionNotice(node.sessionName) ?? t("switchFailed"));
        return;
      }
      void useHerdrStore
        .getState()
        .activateSpace({
          sessionName: node.sessionName,
          workspaceId: node.space.id,
          path: node.space.path,
        })
        .then((result) => {
          if (!result.ok && !result.cancelled)
            setError(result.error ?? t("switchFailed"));
        })
        .catch((error) => setError(String(error)));
      setSelectedAgent(null);
      expand(node.key, collapsed.has(node.key));
    }
  }

  function onKey(event: KeyboardEvent<HTMLButtonElement>, node: TreeNode) {
    if (event.key === "F2" && node.kind === "project") {
      event.preventDefault();
      event.stopPropagation();
      editTrigger.current = event.currentTarget;
      setEditingSpace(node);
      return;
    }
    const index = visible.findIndex((item) => item.key === node.key);
    let next: string | undefined;
    if (event.key === "ArrowDown")
      next = visible[Math.min(index + 1, visible.length - 1)]?.key;
    else if (event.key === "ArrowUp")
      next = visible[Math.max(0, index - 1)]?.key;
    else if (event.key === "Home") next = visible[0]?.key;
    else if (event.key === "End") next = visible.at(-1)?.key;
    else if (event.key === "ArrowRight") {
      if (node.kind === "agent") return;
      if (collapsed.has(node.key)) expand(node.key, true);
      else next = node.children[0]?.key;
    } else if (event.key === "ArrowLeft") {
      if (node.kind !== "agent" && !collapsed.has(node.key))
        expand(node.key, false);
      else next = node.parent;
    } else return;
    event.preventDefault();
    event.stopPropagation();
    if (next) focus(next);
  }

  return (
    <div ref={treeContainer} className="space-tree-panel" aria-busy={movingSpaceKey !== null}>
      <ToggleGroup type="single" value={viewMode} aria-label={t("viewMode", { ns: "spaceNavigation" })}
        className="gap-1 p-2" onValueChange={(value) => {
          if (value !== "spaces" && value !== "agents") return;
          setViewMode(value);
          try { localStorage.setItem("yuzora.sidebar.view", value); } catch { /* In-memory preference remains usable. */ }
        }}>
        <ToggleGroupItem value="spaces" className="flex-1 px-3 py-1">Spaces</ToggleGroupItem>
        <ToggleGroupItem value="agents" className="flex-1 px-3 py-1">Agents</ToggleGroupItem>
      </ToggleGroup>
      <HerdrLauncher
        scope={scopeSession}
        onCreateSpace={createSpace}
        creatingSpace={creatingSpace || creatingTerminal !== null}
        onScopeChange={(name) => {
          setScopeSession(name);
          setSelectedAgent(null);
        }}
      />
      {(error || reorderError) && (
        <p className="space-tree-notice" role="alert">
          {error || reorderError}
        </p>
      )}
      <ScrollArea
        className="space-tree-scroll"
        viewportClassName="[&>div]:block!"
      >
        {[...attention.values()].filter(
          (item) =>
            !(item.kind === "done" && item.seen) &&
            (scopeSession === null || item.sessionName === scopeSession),
        ).length > 0 && (
          <div className="space-tree-attention">
            <strong>
              {t("attention")} ·{" "}
              {
                [...attention.values()].filter(
                  (item) =>
                    !(item.kind === "done" && item.seen) &&
                    (scopeSession === null ||
                      item.sessionName === scopeSession),
                ).length
              }
            </strong>
            {[...attention.values()]
              .filter(
                (item) =>
                  !(item.kind === "done" && item.seen) &&
                  (scopeSession === null || item.sessionName === scopeSession),
              )
              .sort(
                (a, b) =>
                  Number(b.kind === "blocked") - Number(a.kind === "blocked") ||
                  b.updatedAt - a.updatedAt,
              )
              .map((item) => {
                const node = all.find(
                  (node) =>
                    node.agent?.paneId === item.paneId &&
                    node.sessionName === item.sessionName,
                );
                return (
                  <Button
                    key={item.key}
                    variant="ghost"
                    disabled={!node}
                    onClick={() => {
                      if (node) activate(node);
                    }}
                  >
                    {item.title ?? item.displayAgent ?? item.paneId} ·{" "}
                    {t(`status.${item.agentStatus}`)}
                  </Button>
                );
              })}
          </div>
        )}

        <div
          role="tree"
          aria-label={t("treeLabel", { session: scopeSession === null ? "All" : sessionLabel(scopeSession) })}
          aria-description={t("navigationHint")}
          className="space-agent-tree"
          data-design="replica-space-agent-tree"
          data-design-label={t("treeTitle")}
        >
          {shownSessions.map((item) => {
            const sessionNodes = all.filter((node) => node.sessionName === item.name && node.kind !== "agent");
            const expanded = sessionNodes.some((node) => node.kind === "project" && !collapsed.has(node.key));
            const toggleLabel = t(expanded ? "collapseSession" : "expandSession", { session: sessionLabel(item.name) });
            const notice = sessionNotice(item.name);
            return (
              <Fragment key={item.name}>
                <div className="tree-session-heading" data-session-scope={item.name}>
                  <strong title={sessionLabel(item.name)}>{sessionLabel(item.name)}</strong>
                  {viewMode === "spaces" && <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={toggleLabel}
                    title={toggleLabel}
                    aria-expanded={expanded}
                    disabled={sessionNodes.length === 0}
                    onClick={() => {
                      setCollapsed((current) => {
                        const next = new Set(current);
                        for (const node of sessionNodes) {
                          if (expanded) next.add(node.key);
                          else next.delete(node.key);
                        }
                        return next;
                      });
                      setFocusKey(sessionNodes[0]?.key ?? null);
                    }}
                  >
                    {expanded ? <ChevronsDownUp aria-hidden="true" /> : <ChevronsUpDown aria-hidden="true" />}
                  </Button>}
                </div>
                {notice && (
                  <div className="space-tree-notice [overflow-wrap:anywhere]" role="status">
                    <p>{notice}</p>
                    {needsRepair(item.name) && <Button variant="outline" size="sm" onClick={() => repairHost(item.name)}>{t("repairHost")}</Button>}
                  </div>
                )}
                {visible.filter((node) => node.sessionName === item.name).map((node) => {
                  const open = !collapsed.has(node.key),
                    selected = node.key === selectedKey;
                  const containsSelection =
                    node.kind === "project" &&
                    node.sessionName === session &&
                    node.children.some((child) => child.space.id === selectedSpace);
                  return (
                    <div
                      key={node.key}
                      role="none"
                      data-session-scope={node.sessionName}
                      data-space-key={node.kind === "project" ? node.key : undefined}
                      className={`tree-row-shell tree-row-${node.kind} ${draggedSpaceKey ? "select-none" : ""}`}
                      data-dragging={draggedSpaceKey === node.key ? "true" : undefined}
                      data-drop-target={dropTarget?.key === node.key ? (dropTarget.after ? "after" : "before") : undefined}
                      data-current={containsSelection}
                      style={
                        { "--space-color": node.color?.background, touchAction: node.kind === "project" ? "none" : undefined } as CSSProperties
                      }
                      onPointerDownCapture={node.kind === "project" ? (event) => onSpacePointerDown(event, node) : undefined}
                      onMouseDownCapture={node.kind === "project" ? (event) => { if (!window.PointerEvent) onSpacePointerDown(event, node); } : undefined}
                    >
                      <Button
                        ref={(element) => {
                          if (element) refs.current.set(node.key, element);
                          else refs.current.delete(node.key);
                        }}
                        variant="ghost"
                        role="treeitem"
                        aria-level={node.level}
                        aria-posinset={node.position}
                        aria-setsize={node.size}
                        aria-expanded={node.kind === "agent" ? undefined : open}
                        aria-selected={selected}
                        tabIndex={node.key === tabKey ? 0 : -1}
                        aria-label={
                          node.kind === "agent"
                            ? `${node.label} · ${node.agent?.name} · ${t(`status.${node.agent?.status}`)} · ${node.space.branch ?? node.space.label} · ${node.sessionName}`
                            : node.kind === "project"
                              ? `${node.label} · ${t("agentCount", { count: node.count })} · ${node.sessionName}`
                              : `${node.label} · ${node.space.path} · ${node.sessionName}`
                        }
                        aria-description={[
                          sessionNotice(node.sessionName),
                          node.kind === "agent"
                            ? node.agent?.status === "done"
                              ? t("doneMeaning")
                              : t("inspectHint")
                            : `${t("pendingCount", { count: node.pending })} · ${t("folderHint")}`,
                          node.kind === "project" ? t("editShortcut") : null,
                          node.kind === "project" ? t(canReorderSpace(node) ? "dragSpaceHint" : "dragSpaceUnavailable") : null,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                        title={
                          node.kind === "agent"
                            ? t("inspectHint")
                            : (node.space.path ?? undefined)
                        }
                        className={`space-tree-row tree-${node.kind}`}
                        // Native HTML5 drag sessions suppress pointer events in WKWebView.
                        // Pointer Events own the gesture so macOS WebView and touch/pen
                        // input share the same reliable path into HERDR workspace.move.
                        draggable={false}
                        style={{ paddingLeft: 6 + (node.level - 1) * 10 }}
                        onFocus={() => setFocusKey(node.key)}
                        onKeyDown={(event) => onKey(event, node)}
                        onClick={(event) => {
                          if (suppressClickRef.current) {
                            suppressClickRef.current = false;
                            event.preventDefault();
                            event.stopPropagation();
                            return;
                          }
                          activate(node);
                        }}
                        onDragStart={(event) => event.preventDefault()}
                        onContextMenu={
                          (node.kind === "project" || node.kind === "worktree")
                            ? contextMenuHandler({
                                kind: "herdrSpace",
                                sessionName: node.sessionName,
                                workspaceId: node.space.id,
                                label: node.space.label,
                                path: node.space.path ?? null,
                              })
                            : node.kind === "agent" && node.agent?.paneId
                              ? contextMenuHandler({
                                  kind: "herdrPane",
                                  sessionName: node.sessionName,
                                  paneId: node.agent.paneId,
                                  terminalId: node.agent.terminalId ?? null,
                                  tabId: node.agent.tabId ?? null,
                                  workspaceId: node.space.id,
                                  label: node.label,
                                  focusedPaneId:
                                    runtimes[node.sessionName]?.snapshot
                                      ?.focusedPaneId ?? null,
                                })
                              : undefined
                        }
                      >
                        {node.kind === "project" ? (
                          <span
                            className="tree-space-identity"
                            data-avatar={node.avatarMode}
                            style={{
                              background: node.color?.background,
                              color: node.color?.foreground,
                            }}
                            aria-hidden="true"
                          >
                            {node.avatarMode === "character" && node.character ? (
                              <SpaceCharacter character={node.character} portrait />
                            ) : (
                              node.glyph
                            )}
                          </span>
                        ) : node.kind === "worktree" ? (
                          <GitBranch aria-hidden="true" />
                        ) : (
                          <span
                            className="tree-status-dot"
                            data-status={node.agent?.status}
                            aria-hidden="true"
                          />
                        )}
                        <span className="tree-node-label">
                          <span>{node.label}</span>
                          {node.kind === "project" && (
                            <small>
                              {t("spaceSummary", {
                                branches: node.children.length,
                                agents: node.count,
                              })}
                            </small>
                          )}
                          {node.kind === "agent" && (
                            <span className="tree-agent-meta">
                              <small>{node.agent?.name}</small>
                              <span className="tree-agent-status">
                                {t(`status.${node.agent?.status}`)}
                              </span>
                            </span>
                          )}
                        </span>
                        {node.kind === "worktree" && (
                          <span
                            className="tree-node-count"
                            data-pending={node.pending > 0}
                          >
                            {node.pending
                              ? t("pendingCount", { count: node.pending })
                              : node.count}
                          </span>
                        )}
                      </Button>
                      {node.kind === "worktree" && (
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="tree-add-terminal shrink-0"
                          tabIndex={node.key === tabKey ? 0 : -1}
                          aria-label={t("addTerminalToBranch", {
                            branch: node.label,
                            session: sessionLabel(node.sessionName),
                          })}
                          title={canAddTerminal(node.sessionName)
                            ? t("addTerminalToBranch", { branch: node.label, session: sessionLabel(node.sessionName) })
                            : t("terminalUnavailable")}
                          disabled={creatingTerminal !== null || creatingSpace || !canAddTerminal(node.sessionName)}
                          aria-busy={creatingTerminal === node.key}
                          onClick={() => void addTerminal(node)}
                        >
                          <Plus aria-hidden="true" />
                        </Button>
                      )}
                      {node.kind === "agent" && (
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          tabIndex={node.key === tabKey ? 0 : -1}
                          aria-label={t("inspectAgent", { name: node.label })}
                          onClick={(event) => {
                            inspectorTrigger.current = event.currentTarget;
                            setFocusKey(node.key);
                            setInspected(node.agent!);
                          }}
                        >
                          <Info aria-hidden="true" />
                        </Button>
                      )}
                      {node.kind === "project" && (
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="tree-edit-space"
                          aria-label={t("editSpaceNamed", {
                            name: node.label,
                            session: node.sessionName,
                          })}
                          tabIndex={node.key === tabKey ? 0 : -1}
                          onClick={(event) => {
                            if (suppressClickRef.current) {
                              suppressClickRef.current = false;
                              event.preventDefault();
                              event.stopPropagation();
                              return;
                            }
                            editTrigger.current = event.currentTarget;
                            setFocusKey(node.key);
                            setEditingSpace(node);
                          }}
                        >
                          <EllipsisVertical aria-hidden="true" />
                        </Button>
                      )}
                    </div>
                  );
                })}
                {!notice && sessionNodes.length === 0 && (
                  <div className="space-tree-empty [overflow-wrap:anywhere]" role="status">
                    <p>{sessionLabel(item.name)}: {t("empty")}</p>
                    {runtimes[item.name]?.capabilities?.api.workspaceCreate && runtimes[item.name]?.capabilities?.terminal.create && (
                      <>
                        <p>{t("firstTerminalHint")}</p>
                        <Button variant="outline" size="sm" className="h-auto whitespace-normal" disabled={creatingSpace || creatingTerminal !== null} onClick={() => void createSpace(item.name)}>
                          {creatingSpace ? t("openingSpace") : t("openFolderAndCreateSpace")}
                        </Button>
                      </>
                    )}
                  </div>
                )}
              </Fragment>
            );
          })}
        </div>
        {!shownSessions.length && <div className="space-tree-empty"><p>{t("noRunningSessions")}</p><Button variant="outline" size="sm" onClick={() => repairHost()}>{t("runtimeSettings")}</Button></div>}
        {visible.filter(
          (node) =>
            node.kind === "worktree" &&
            !collapsed.has(node.key) &&
            node.count === 0,
        ).length > 0 && (
          <p className="space-tree-empty">{t("emptyWorktrees")}</p>
        )}
      </ScrollArea>
      <HerdrAgentInspector
        open={inspected !== null}
        agent={inspected}
        onOpenChange={(open) => {
          if (!open) setInspected(null);
        }}
        returnFocusRef={inspectorTrigger}
      />
      {editingSpace && (
        <SpaceAppearanceDialog
          identityKey={editingSpace.identityKey!}
          path={
            editingSpace.space.repoRoot ??
            editingSpace.space.path ??
            editingSpace.space.label
          }
          onClose={() => setEditingSpace(null)}
          returnFocusRef={editTrigger}
        />
      )}
    </div>
  );
}
