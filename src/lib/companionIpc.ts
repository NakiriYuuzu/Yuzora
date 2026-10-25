import { invoke } from "./ipc"

// The command shapes are pinned by the Rust test
// `companion_ui_fixtures_match_the_serialized_shapes` (src/test/companionFixtures.json).

export const COMPANION_EVENT = "companion://event"

export type CompanionWindow = "1h" | "8h" | "untilExit"

export type CompanionStatus =
  | { state: "disabled" }
  | { state: "starting" }
  | { state: "running"; port: number }
  | { state: "stopping" }
  | { state: "failed"; code: string }

/** The tailnet node this desktop last saw: its name and the account that owns it. */
export interface NetIdentity { dnsName: string; login: string }

export interface CompanionSettings {
  version: number
  gatewayPort: number
  duration: CompanionWindow
  sessionAllowlist: string[]
  fullSessionEnabled: boolean
  net: NetIdentity | null
  experimental: boolean
}

/** The embedded Tailscale node (`yuzora-companion-net`). `dnsName` and `login` are the last seen ones while it is stopped. In `needsApproval`, `dnsName` carries the name of the machine that waits for approval (what the node reported; a remembered name only when it reported none). */
export interface NetOverview {
  /** `needsApproval`: signed in, but a tailnet admin has not approved this device yet. */
  state: "stopped" | "starting" | "needsLogin" | "needsApproval" | "running" | "failed"
  /** Why the node runs: sign-in, serving the Companion, or signing out. Null while it does not. */
  mode: "login" | "enable" | "logout" | null
  /** The page to sign in on, while `needsLogin`. */
  authUrl: string | null
  dnsName: string | null
  login: string | null
  /** Unknown until the node runs; false when the tailnet has no HTTPS certificates. */
  httpsCapable: boolean | null
  cert: { state: "pending" | "ready" | "error"; notAfter: string | null; error: string | null } | null
  serving: boolean
  /** The node was seen running and has not been asked to sign in again since. */
  loggedIn: boolean
  /** A closed set of codes, resolved like command errors. */
  error: string | null
}

/** Web Push: devices with a stored subscription. */
export interface PushOverview { available: boolean; subscribed: string[] }

export interface CompanionOverview {
  status: CompanionStatus
  settings: CompanionSettings
  /** A one-year backstop for `untilExit`; show `settings.duration` instead. */
  enableUntil: string | null
  /** False when another Yuzora instance owns the Companion. */
  owner: boolean
  net: NetOverview
  push: PushOverview
}

export interface CompanionGrants { control: { until: string } | null; full: boolean }

export interface CompanionDevice {
  deviceId: string
  label: string
  grants: CompanionGrants
  locked: boolean
  createdAt: string
  lastSeen: string
}

export interface CompanionPairingQr {
  pairingId: string
  /** Encodes the pairing secret: render it, never store or log it. */
  qrSvg: string
  expiresAt: string
}

/** The address an already paired phone opens; no secret in it. */
export interface CompanionPhoneLink { url: string; qrSvg: string }

export interface CompanionClaim {
  pairingId: string
  deviceLabel: string
  sas: string
  via: string
  tailscaleLogin: string
  userAgent: string
}

/** One action a phone took. `summary` may carry the start of a prompt: never log it whole. */
export interface CompanionAudit {
  deviceId: string
  peerId: number
  scope: "view" | "control" | "full"
  action: string
  session: string
  target: string | null
  summary: Record<string, unknown>
  at: string
  reason?: string
  outcome?: string
}

/** A phone's control stream opened (seq 1) or closed (seq 2); `stream` is unique per gateway run. */
export interface CompanionControlState {
  deviceId: string
  peerId: number
  stream: string
  seq: number
  session: string
  target: string
  takeover: boolean
  open: boolean
}

/** A full-session stream (scope `full`) opened (seq 1) or closed (seq 2); it has no pane. */
export interface CompanionFullState {
  deviceId: string
  peerId: number
  stream: string
  seq: number
  session: string
  open: boolean
}

/** The `companion://event` payloads the UI acts on; others are ignored. */
export type CompanionEvent =
  | { type: "status"; status: CompanionStatus }
  | { type: "device.registered" | "device.updated"; device: CompanionDevice }
  | { type: "device.revoked"; deviceId: string }
  | ({ type: "pairing.claimed" } & CompanionClaim)
  | { type: "pairing.state"; pairingId: string; state: "approved" | "rejected" | "expired"; reason: string }
  | { type: "command.deferred"; command: string; target: string }
  | { type: "error"; code: string }
  | ({ type: "audit" } & CompanionAudit)
  | ({ type: "control.state" } & CompanionControlState)
  | ({ type: "full.state" } & CompanionFullState)
  | { type: "session.opened"; peerId: number; deviceId: string }
  | { type: "session.closed"; peerId: number; deviceId: string; reason: string }
  | { type: "net"; net: NetOverview }
  | { type: "push"; push: PushOverview }
  | { type: "audit.dropped"; lost: number; lostTightening: number; rejectedOmitted: number; since: string }

export function companionStatus(): Promise<CompanionOverview> {
  return invoke("companion_status")
}

export function companionExperimentalSet(enabled: boolean): Promise<CompanionOverview> {
  return invoke("companion_experimental_set", { enabled })
}

/** Starts the node and the gateway behind it; resolves once both serve. Call only after the user confirmed. */
export function companionEnable(window: CompanionWindow): Promise<CompanionStatus> {
  return invoke("companion_enable", { window, confirm: true })
}

export function companionDisable(): Promise<void> {
  return invoke("companion_disable")
}

/** Starts the node to sign in; follow it through `net` events. */
export function companionNetLogin(): Promise<NetOverview> {
  return invoke("companion_net_login")
}

/** Gives up a sign-in in progress. */
export function companionNetCancel(): Promise<NetOverview> {
  return invoke("companion_net_cancel")
}

/** Signs the node out of the tailnet and forgets it. */
export function companionNetLogout(): Promise<NetOverview> {
  return invoke("companion_net_logout")
}

export function companionPairingCreate(): Promise<CompanionPairingQr> {
  return invoke("companion_pairing_create", { path: "tailscale" })
}

export function companionPhoneLink(): Promise<CompanionPhoneLink> {
  return invoke("companion_phone_link")
}

export function companionPairingDecide(pairingId: string, decision: "approve" | "reject" | "cancel"): Promise<void> {
  return invoke("companion_pairing_decide", { pairingId, decision })
}

export function companionDevices(): Promise<CompanionDevice[]> {
  return invoke("companion_devices")
}

/** A phone connected to the gateway now. */
export interface CompanionPeer { peerId: number; deviceId: string }
/** An open control or full stream; `stream` is the key `control.state` / `full.state` use. */
export interface CompanionOpenStream {
  stream: string
  deviceId: string
  peerId: number
  session: string
  scope: "control" | "full"
  /** The pane a control stream drives. */
  target?: string
}
/** The gateway's own list of who is connected and who drives what, unlike the best effort `session.*` events. */
export interface CompanionPeers { peers: CompanionPeer[]; streams: CompanionOpenStream[] }

export function companionPeers(): Promise<CompanionPeers> {
  return invoke("companion_peers")
}

export function companionDeviceUpdate(deviceId: string, change: { label: string } | { grants: CompanionGrants }): Promise<CompanionDevice> {
  return invoke("companion_device_update", { deviceId, ...change })
}

export function companionDeviceUnlock(deviceId: string): Promise<CompanionDevice> {
  return invoke("companion_device_unlock", { deviceId })
}

export function companionDeviceRevoke(deviceId: string): Promise<void> {
  return invoke("companion_device_revoke", { deviceId })
}

/** The gateway reads the allowlist at start: `restartRequired` while one runs. */
export function companionSessionsSet(sessions: string[]): Promise<{ restartRequired: boolean }> {
  return invoke("companion_sessions_set", { sessions })
}

/** A local Space the gateway may read git from: its Session, its id, and its folder. */
export interface CompanionSpaceRoot { session: string; workspaceId: string; root: string }

export function companionSpaceRootsSet(roots: CompanionSpaceRoot[]): Promise<void> {
  return invoke("companion_space_roots_set", { roots })
}
