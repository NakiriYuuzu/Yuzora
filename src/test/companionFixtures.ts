import fixtures from "./companionFixtures.json"
import type { CompanionDevice, CompanionEvent, CompanionOverview, CompanionPairingQr, CompanionPhoneLink, CompanionStatus, NetOverview, PushOverview } from "@/lib/companionIpc"

/** What the Rust commands serialize, pinned by `companion_ui_fixtures_match_the_serialized_shapes`. */
export const companionFixtures = fixtures as unknown as {
  overviewOff: CompanionOverview
  overviewRunning: CompanionOverview
  statusFailed: CompanionStatus
  statusStarting: CompanionStatus
  statusStopping: CompanionStatus
  netStopped: NetOverview
  netLoggedOut: NetOverview
  netEnabling: NetOverview
  netSigningOut: NetOverview
  netNeedsLogin: NetOverview
  netNeedsApproval: NetOverview
  netCertPending: NetOverview
  netRunning: NetOverview
  pushOverview: PushOverview
  pushEvent: Extract<CompanionEvent, { type: "push" }>
  device: CompanionDevice
  pairingQr: CompanionPairingQr
  phoneLink: CompanionPhoneLink
  sessionsSet: { restartRequired: boolean }
}

/** `{ reject }` makes the mocked command fail with that message, as Tauri rejects with a string. */
export type CompanionReplies = Record<string, unknown | { reject: string } | ((args: Record<string, unknown>) => unknown)>

export function companionInvoke(replies: () => CompanionReplies) {
  return async (command: string, args: Record<string, unknown> = {}) => {
    const reply = replies()[command]
    if (typeof reply === "function") return reply(args)
    if (reply && typeof reply === "object" && "reject" in reply) throw (reply as { reject: string }).reject
    return reply
  }
}
