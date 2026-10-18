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

export interface CompanionAppliedServe { httpsPort: number; gatewayPort: number; host: string; login: string }

export interface CompanionSettings {
  version: number
  httpsPort: number
  gatewayPort: number
  duration: CompanionWindow
  sessionAllowlist: string[]
  fullSessionEnabled: boolean
  /** A Serve handler this desktop applied and has not confirmed removed. */
  applied: CompanionAppliedServe | null
  experimental: boolean
}

export interface CompanionOverview {
  status: CompanionStatus
  settings: CompanionSettings
  /** A one-year backstop for `untilExit`; show `settings.duration` instead. */
  enableUntil: string | null
  /** False when another Yuzora instance owns the Companion. */
  owner: boolean
}

export type TailscaleRefusal = "notRunning" | "noLogin" | "tagged" | "httpsNotReady" | "funnel" | "portTaken"

export interface TailscaleProbe {
  cli: { path: string; variant: "macAppStore" | "macStandalone" | "cli" } | null
  node: {
    version: string
    backendState: string
    dnsName: string | null
    login: string | null
    tagged: boolean
    magicDns: boolean
    httpsReady: boolean
  } | null
  serve: { port: "free" | "ours" | "shared" | "taken"; foreground: boolean; funnel: boolean; otherPorts: number[] } | null
  refusal: TailscaleRefusal | null
  httpsPort: number
  gatewayPort: number
  /** The exact command enable runs; shown verbatim before confirming. */
  command: string
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
  | { type: "audit.dropped"; lost: number; lostTightening: number; rejectedOmitted: number; since: string }

export function companionStatus(): Promise<CompanionOverview> {
  return invoke("companion_status")
}

export function companionExperimentalSet(enabled: boolean): Promise<CompanionOverview> {
  return invoke("companion_experimental_set", { enabled })
}

/** Runs `tailscale serve`: call only after the user confirmed `probe.command`. */
export function companionEnable(window: CompanionWindow): Promise<CompanionStatus> {
  return invoke("companion_enable", { window, confirm: true })
}

export function companionDisable(): Promise<void> {
  return invoke("companion_disable")
}

export function companionTailscaleProbe(): Promise<TailscaleProbe> {
  return invoke("companion_tailscale_probe")
}

export function companionTailscaleRemove(): Promise<void> {
  return invoke("companion_tailscale_remove")
}

/** Only for a record whose node this desktop no longer reaches; the handler may remain there. */
export function companionTailscaleForget(): Promise<void> {
  return invoke("companion_tailscale_forget")
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
