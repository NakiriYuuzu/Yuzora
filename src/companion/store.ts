import { create } from "zustand"
import type { PairingLink } from "./lib/pairing"
import type { Hello, Scope, SessionInfo, Snapshot } from "./lib/protocol"
import type { SocketState } from "./lib/rpc"

export type Screen =
  | { name: "pair"; link: PairingLink | "invalid" | null }
  | { name: "login"; reason?: "expired" | "registered" | "logged-out" }
  | { name: "home" }
  | { name: "pane"; session: string; pane: string }

export type Notice = { tone: "info" | "warning" | "error"; key: string; values?: Record<string, string | number> }
export type Elevation = { scope: Scope; until: string; at: number }

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
  elevation: Elevation | null
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
  elevation: null,
  notice: null,
}))

export const navigate = (screen: Screen) => useCompanion.setState({ screen })
export const notify = (notice: Notice | null) => useCompanion.setState({ notice })

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
