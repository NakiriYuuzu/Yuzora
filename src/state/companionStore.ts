import { create } from "zustand"
import {
  companionDevices,
  companionStatus,
  type CompanionClaim,
  type CompanionDevice,
  type CompanionEvent,
  type CompanionOverview,
} from "@/lib/companionIpc"

/** A phone's open control or full stream; `target` is the pane a control stream drives. */
export interface CompanionStream {
  deviceId: string
  peerId: number
  session: string
  target?: string
  scope: "control" | "full"
}

interface CompanionState {
  overview: CompanionOverview | null
  devices: CompanionDevice[]
  /** Phones connected now: peer id to device id. Empty whenever the gateway is not running. */
  peers: Record<number, string>
  /** Open control and full streams by stream key; empty whenever the gateway is not running. */
  streams: Record<string, CompanionStream>
  /** The newest seq seen per stream, so an open that arrives after its close cannot revive it. */
  streamSeq: Record<string, number>
  /** When each phone last read Git (the audit's `at`), kept for as long as the app runs. */
  lastGitRead: Record<string, string>
  /** Claims waiting for the desktop's decision, oldest first. */
  claims: CompanionClaim[]
  /** The final `pairing.state` per pairing id, so an open QR dialog can close. */
  settled: Record<string, string>
  /** The user started enabling: the welcome host greets them once the Companion serves. */
  welcomePending: boolean
  /** Why the last enable failed. Kept here so a card opened after it still shows it; cleared when the next enable starts or the Companion runs. */
  enableError: string | null
  refresh: () => Promise<void>
  setOverview: (overview: CompanionOverview) => void
  applyEvent: (event: CompanionEvent) => void
  dropClaim: (pairingId: string) => void
  setWelcomePending: (pending: boolean) => void
  setEnableError: (error: string | null) => void
}

// Bumped by every event that changes what `refresh` reads: a refresh that
// raced one reads again rather than overwrite the newer state.
let eventSeq = 0
let refreshing: Promise<void> | null = null
// A refresh that never read a quiet moment tries again later, a few times.
const RETRY_MS = 1000
const MAX_RETRIES = 3
let retries = 0
let retryTimer: ReturnType<typeof setTimeout> | null = null

/** A running Companion has no enable failure left to explain. */
const settledEnable = (overview: CompanionOverview) => overview.status.state === "running" ? { enableError: null } : {}

export const useCompanionStore = create<CompanionState>((set, get) => ({
  overview: null,
  devices: [],
  peers: {},
  streams: {},
  streamSeq: {},
  lastGitRead: {},
  claims: [],
  settled: {},
  welcomePending: false,
  enableError: null,
  refresh() {
    refreshing ??= (async () => {
      try {
        for (let attempt = 0; ; attempt++) {
          const seq = eventSeq
          const [overview, devices] = await Promise.all([companionStatus(), companionDevices()])
          if (seq === eventSeq) {
            retries = 0
            set({ overview, devices, ...settledEnable(overview) })
            return
          }
          // Never commit a read an event overtook: it would undo that event.
          if (attempt === 2) {
            if (retries++ < MAX_RETRIES && !retryTimer) {
              retryTimer = setTimeout(() => {
                retryTimer = null
                void get().refresh().catch(() => undefined)
              }, RETRY_MS)
            }
            return
          }
        }
      } finally {
        refreshing = null
      }
    })()
    return refreshing
  },
  setOverview(overview) {
    eventSeq++
    set({ overview, ...settledEnable(overview) })
  },
  applyEvent(event) {
    switch (event.type) {
      case "status": {
        eventSeq++
        const overview = get().overview
        if (overview) set({ overview: { ...overview, status: event.status } })
        if (event.status.state === "running") set({ enableError: null })
        if (event.status.state === "disabled" || event.status.state === "failed") {
          // A stopped gateway settles no claim: nothing is left to approve.
          set({ claims: [] })
          // enableUntil is gone.
          void get().refresh().catch(() => undefined)
        }
        // Nobody is connected to a gateway that is not running.
        if (event.status.state !== "running") set({ peers: {}, streams: {}, streamSeq: {} })
        return
      }
      case "net": {
        eventSeq++
        const overview = get().overview
        if (overview) set({ overview: { ...overview, net: event.net } })
        return
      }
      case "push": {
        eventSeq++
        const overview = get().overview
        if (overview) set({ overview: { ...overview, push: event.push } })
        return
      }
      case "session.opened":
        set({ peers: { ...get().peers, [event.peerId]: event.deviceId } })
        return
      case "session.closed": {
        const { [event.peerId]: _closed, ...peers } = get().peers
        const streams = Object.fromEntries(Object.entries(get().streams).filter(([, stream]) => stream.peerId !== event.peerId))
        set({ peers, streams })
        return
      }
      case "control.state":
      case "full.state": {
        if (event.seq <= (get().streamSeq[event.stream] ?? 0)) return
        const { [event.stream]: _closed, ...streams } = get().streams
        const stream: CompanionStream = event.type === "control.state"
          ? { deviceId: event.deviceId, peerId: event.peerId, session: event.session, target: event.target, scope: "control" }
          : { deviceId: event.deviceId, peerId: event.peerId, session: event.session, scope: "full" }
        set({
          streamSeq: { ...get().streamSeq, [event.stream]: event.seq },
          streams: event.open ? { ...streams, [event.stream]: stream } : streams,
        })
        return
      }
      case "audit":
        if (event.action === "git.read") set({ lastGitRead: { ...get().lastGitRead, [event.deviceId]: event.at } })
        return
      case "device.registered":
      case "device.updated": {
        eventSeq++
        const devices = get().devices.filter(device => device.deviceId !== event.device.deviceId)
        set({ devices: [...devices, event.device] })
        return
      }
      case "device.revoked":
        eventSeq++
        set({ devices: get().devices.filter(device => device.deviceId !== event.deviceId) })
        return
      case "command.deferred":
        // Applied and stored, but the gateway could not report the device yet.
        eventSeq++
        void get().refresh().catch(() => undefined)
        return
      case "pairing.claimed": {
        const { type: _type, ...claim } = event
        set({ claims: [...get().claims.filter(item => item.pairingId !== claim.pairingId), claim] })
        return
      }
      case "pairing.state":
        set({
          settled: { ...get().settled, [event.pairingId]: event.state },
          claims: get().claims.filter(item => item.pairingId !== event.pairingId),
        })
        return
    }
  },
  dropClaim(pairingId) {
    set({ claims: get().claims.filter(item => item.pairingId !== pairingId) })
  },
  setWelcomePending(pending) {
    set({ welcomePending: pending })
  },
  setEnableError(error) {
    set({ enableError: error })
  },
}))
