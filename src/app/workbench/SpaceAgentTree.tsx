import { contextMenuHandler } from "@/state/contextMenuStore";
import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import {
  ChevronsDownUp,
  GitBranch,
  EllipsisVertical,
  Info,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { HerdrAgentInspector } from "@/app/workbench/HerdrAgentInspector";
import { resolveProjectPresentation } from "@/app/workbench/projectPresentation";
import { useHerdrStore } from "@/state/herdrStore";
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces";

import { sortHerdrAgentsByUrgency } from "@/lib/herdrAgents";
import type { HerdrAgentInfo, HerdrSpaceInfo } from "@/lib/herdrTypes";
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider";
import { spacePresentationKey } from "./spaceTreeIdentity";
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
    return [named?.hostLabel, named?.name ?? parseRuntimeScope(name).sessionName]
      .filter(Boolean).join(" · ");
  }

  const roots = useMemo(
    () =>
      sessions
        .filter((item) => scopeSession === null || item.name === scopeSession)
        .flatMap(({ name: sessionName }) => {
          const snapshot = runtimes[sessionName]?.snapshot;
          const groups = new Map<string, HerdrSpaceInfo[]>();
          for (const space of snapshot?.spaces ?? []) {
            const key = space.repoKey ?? space.id;
            groups.set(key, [...(groups.get(key) ?? []), space]);
          }
          return [...groups.entries()].map(
            ([group, spaces], index): TreeNode => {
              const first = spaces[0],
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
  const all = roots.flatMap((root) => [
    root,
    ...root.children.flatMap((branch) => [branch, ...branch.children]),
  ]);
  const visible = roots.flatMap((root) => [
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
    <div className="space-tree-panel">
      <HerdrLauncher
        scope={scopeSession}
        onScopeChange={(name) => {
          setScopeSession(name);
          setSelectedAgent(null);
        }}
      />
      <div className="tree-browse-toolbar">
        <span>{scopeSession === null ? t("allHint")
          : !sessionNotice(scopeSession) ? t("loaded")
          : runtimes[scopeSession]?.snapshot ? t("stale")
          : !runtimes[scopeSession] || ["idle", "connecting"].includes(runtimes[scopeSession].connectionState) ? t("loading")
          : t("unavailable")}</span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("collapseAll")}
          onClick={() => {
            setCollapsed(
              new Set(
                all
                  .filter((node) => node.kind !== "agent")
                  .map((node) => node.key),
              ),
            );
            setFocusKey(roots[0]?.key ?? null);
          }}
        >
          <ChevronsDownUp />
        </Button>
      </div>
      {scopeSession !== null && sessionNotice(scopeSession) && (
        <p className="space-tree-notice [overflow-wrap:anywhere]" role="status">
          {sessionNotice(scopeSession)}
        </p>
      )}
      {error && (
        <p className="space-tree-notice" role="alert">
          {error}
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
          {visible.map((node, index) => {
            const open = !collapsed.has(node.key),
              selected = node.key === selectedKey;
            const containsSelection =
              node.kind === "project" &&
              node.sessionName === session &&
              node.children.some((child) => child.space.id === selectedSpace);
            return (
              <Fragment key={node.key}>
                {scopeSession === null &&
                  visible[index - 1]?.sessionName !== node.sessionName && (
                    <p className="tree-session-heading">
                      <strong>
                        {[
                          rawSessions.find(
                            (item) => sessionScope(item) === node.sessionName,
                          )?.hostLabel,
                          parseRuntimeScope(node.sessionName).sessionName,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </strong>
                      <span>
                        {sessionNotice(node.sessionName) ?? t("loaded")}
                      </span>
                    </p>
                  )}
                <div
                  role="none"
                  className={`tree-row-shell tree-row-${node.kind}`}
                  data-current={containsSelection}
                  style={
                    { "--space-color": node.color?.background } as CSSProperties
                  }
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
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                    title={
                      node.kind === "agent"
                        ? t("inspectHint")
                        : (node.space.path ?? undefined)
                    }
                    className={`space-tree-row tree-${node.kind}`}
                    style={{ paddingLeft: 6 + (node.level - 1) * 10 }}
                    onFocus={() => setFocusKey(node.key)}
                    onKeyDown={(event) => onKey(event, node)}
                    onClick={() => activate(node)}
                    onContextMenu={
                      node.kind === "worktree"
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
                        editTrigger.current = event.currentTarget;
                        setFocusKey(node.key);
                        setEditingSpace(node);
                      }}
                    >
                      <EllipsisVertical aria-hidden="true" />
                    </Button>
                  )}
                </div>
              </Fragment>
            );
          })}
        </div>
        {!shownSessions.length && <p className="space-tree-empty">{t("noRunningSessions")}</p>}
        {shownSessions
          .filter((item) => !runtimes[item.name]?.snapshot?.spaces.length)
          .filter(() => scopeSession === null || !sessionNotice(scopeSession))
          .map((item) => (
            <p key={item.name} className="space-tree-empty [overflow-wrap:anywhere]" role="status">
              {sessionLabel(item.name)}: {sessionNotice(item.name) ?? t("empty")}
            </p>
          ))}
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
