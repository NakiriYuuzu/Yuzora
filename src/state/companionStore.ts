import { create } from "zustand"
import {
  companionDevices,
  companionStatus,
  type CompanionClaim,
  type CompanionDevice,
  type CompanionEvent,
  type CompanionOverview,
} from "@/lib/companionIpc"

interface CompanionState {
  overview: CompanionOverview | null
  devices: CompanionDevice[]
  /** Claims waiting for the desktop's decision, oldest first. */
  claims: CompanionClaim[]
  /** The final `pairing.state` per pairing id, so an open QR dialog can close. */
  settled: Record<string, string>
  refresh: () => Promise<void>
  setOverview: (overview: CompanionOverview) => void
  applyEvent: (event: CompanionEvent) => void
  dropClaim: (pairingId: string) => void
}

// Bumped by every event that changes what `refresh` reads: a refresh that
// raced one reads again rather than overwrite the newer state.
let eventSeq = 0
let refreshing: Promise<void> | null = null

export const useCompanionStore = create<CompanionState>((set, get) => ({
  overview: null,
  devices: [],
  claims: [],
  settled: {},
  refresh() {
    refreshing ??= (async () => {
      try {
        for (let attempt = 0; ; attempt++) {
          const seq = eventSeq
          const [overview, devices] = await Promise.all([companionStatus(), companionDevices()])
          if (seq === eventSeq || attempt === 2) {
            set({ overview, devices })
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
    set({ overview })
  },
  applyEvent(event) {
    switch (event.type) {
      case "status": {
        eventSeq++
        const overview = get().overview
        if (overview) set({ overview: { ...overview, status: event.status } })
        if (event.status.state === "disabled" || event.status.state === "failed") {
          // A stopped gateway settles no claim: nothing is left to approve.
          set({ claims: [] })
          // The stop hook may have cleared the Serve record, and enableUntil is gone.
          void get().refresh().catch(() => undefined)
        }
        return
      }
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
}))
