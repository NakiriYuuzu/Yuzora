import { create } from "zustand"
import type { PaneTarget } from "./lib/deepLink"
import type { PairingLink } from "./lib/pairing"
import type { DiffSource, GitBranches, GitCommit, GitLog, GitStatus, Hello, Scope, SessionInfo, Snapshot } from "./lib/protocol"
import type { SocketState } from "./lib/rpc"

export type Screen =
  | { name: "pair"; link: PairingLink | "invalid" | null }
  | { name: "login"; reason?: "expired" | "registered" | "logged-out" }
  | { name: "home" }
  | { name: "pane"; session: string; pane: string }
  | { name: "space"; session: string; workspaceId: string }
  | { name: "diff"; session: string; workspaceId: string; source: DiffSource }
  | { name: "commit"; session: string; workspaceId: string; hash: string }

export type Notice = { tone: "info" | "warning" | "error"; key: string; values?: Record<string, string | number> }
export type Elevation = { scope: Scope; until: string; at: number }

/** What the phone has read of one Space's repository; every part fills in as its view opens. */
export type SpaceGit = {
  /** The repository these reads are of (`gitGeneration` of the snapshot they were made under); a different one drops them all. */
  generation?: string
  status?: GitStatus
  branches?: GitBranches
  log?: GitLog
  /** Commits are immutable, so a loaded one is never fetched again. */
  commits: Record<string, GitCommit>
}
/**
 * The Git reads without those of `session`'s Spaces that `snapshot` no longer
 * shows: gone, not shared any more, or mapped to another repository since they
 * were read. `null` (the session is gone) drops all of the session's.
 */
export function pruneGit(git: Record<string, SpaceGit>, session: string, snapshot: Snapshot | null): Record<string, SpaceGit> {
  const prefix = `${session}\0`
  return Object.fromEntries(
    Object.entries(git).filter(([key, entry]) => {
      if (!key.startsWith(prefix)) return true
      const space = snapshot?.workspaces.find((item) => item.id === key.slice(prefix.length))
      return Boolean(space?.git) && space?.gitGeneration === entry.generation
    }),
  )
}
export type GitView = "changes" | "history" | "branches"
export type SpaceTab = "agents" | "git"
export type SpaceView = { tab: SpaceTab; view: GitView }

type State = {
  screen: Screen
  binding: string | null
  socket: SocketState
  hello: Hello | null
  /** Server clock minus the phone's. */
  offset: number
  /** Bumps on every authenticated (re)connection; streams reopen on change. */
  epoch: number
  sessions: SessionInfo[] | null
  snapshots: Record<string, Snapshot>
  sessionErrors: Record<string, string>
  /** Per Space (`spaceKey`): its repository reads, and which tab and Git view it was left on. */
  git: Record<string, SpaceGit>
  spaceView: Record<string, SpaceView>
  elevation: Elevation | null
  /** A pane a notification pointed at; opened once its snapshot is known, dropped if it is gone. */
  pendingPane: PaneTarget | null
  notice: Notice | null
}

export const useCompanion = create<State>(() => ({
  screen: { name: "pair", link: null },
  binding: null,
  socket: "idle",
  hello: null,
  offset: 0,
  epoch: 0,
  sessions: null,
  snapshots: {},
  sessionErrors: {},
  git: {},
  spaceView: {},
  elevation: null,
  pendingPane: null,
  notice: null,
}))

// The system back gesture walks the browser's history, so screens opened from
// another screen get an entry each. An entry's state is only its depth and the
// page load it belongs to: the screens themselves stay in memory (a pairing
// link or any other secret never reaches history, and a reload starts again
// from a root screen).
const pageLoad = Date.now()
let trail: Screen[] = []
let depth = 0
// How far each screen below the current one was scrolled when it was left.
let offsets: number[] = []

/** Switches to a root screen (pair, login, home): no history entry is added. */
export function navigate(screen: Screen): void {
  trail = [screen]
  offsets = []
  depth = 0
  history.replaceState({ depth, load: pageLoad }, "")
  useCompanion.setState({ screen })
}

/** Opens a screen on top of the current one; back returns to it. */
export function open(screen: Screen): void {
  trail = [...trail.slice(0, depth + 1), screen]
  offsets = [...offsets.slice(0, depth), window.scrollY]
  depth += 1
  history.pushState({ depth, load: pageLoad }, "")
  useCompanion.setState({ screen })
}

/** Where to scroll the current screen to: the top, unless it is one the user came back to. */
export const savedScroll = () => offsets[depth] ?? 0

/** The in-app back button: the same step as the system gesture. */
export function goBack(): void {
  if (depth > 0) history.back()
  else navigate({ name: "home" })
}

/** `popstate`: shows the screen of the entry the browser moved to. */
export function restoreScreen(event: PopStateEvent): void {
  const entry = event.state as { depth?: unknown; load?: unknown } | null
  const target = entry?.depth
  // Entries from before a reload or a root switch have no screen left.
  if (entry?.load !== pageLoad || typeof target !== "number" || !trail[target]) return
  depth = target
  useCompanion.setState({ screen: trail[target] })
}

export const notify = (notice: Notice | null) => useCompanion.setState({ notice })
export const setSpaceView = (key: string, change: Partial<SpaceView>) =>
  useCompanion.setState((state) => ({ spaceView: { ...state.spaceView, [key]: { ...(state.spaceView[key] ?? { tab: "agents", view: "changes" }), ...change } } }))

// The binding pairs with the HttpOnly cookie. It lives for this tab only and
// never reaches localStorage, the URL or logs.
const BINDING_KEY = "yzc.binding"
// Non-secret hints only.
const PAIRED_KEY = "yzc.paired"
const LABEL_KEY = "yzc.label"

export const storage = {
  binding: () => sessionStorage.getItem(BINDING_KEY),
  setBinding(binding: string | null) {
    if (binding) sessionStorage.setItem(BINDING_KEY, binding)
    else sessionStorage.removeItem(BINDING_KEY)
    useCompanion.setState({ binding })
  },
  paired: () => localStorage.getItem(PAIRED_KEY) === "1",
  setPaired: () => localStorage.setItem(PAIRED_KEY, "1"),
  label: () => localStorage.getItem(LABEL_KEY),
  setLabel: (label: string) => localStorage.setItem(LABEL_KEY, label),
}
