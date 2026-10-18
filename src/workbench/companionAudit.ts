import { toast } from "sonner"
import { isPermissionGranted, sendNotification } from "@tauri-apps/plugin-notification"
import type { CompanionEvent } from "@/lib/companionIpc"
import { sessionScope } from "@/lib/herdrProvider"
import i18n from "@/lib/i18n"
import { invoke } from "@/lib/ipc"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"

// Counts and fixed codes only. `agent.prompt` also carries the prompt's first
// characters, and Logs persist: an allowlist keeps any content field out.
const SUMMARY_FIELDS = ["length", "bytes", "keys", "count", "accepted", "takeover", "resizeConfirmed", "reason", "outcome", "phase"]
// Writes a phone sends to a pane; the notice updates in place per device and pane.
const WRITES = new Set(["agent.prompt", "pane.sendKeys", "terminal.input"])

function deviceLabel(deviceId: string): string {
  return useCompanionStore.getState().devices.find(device => device.deviceId === deviceId)?.label
    ?? i18n.t("companion:audit.unknownDevice")
}

/** The agent's name from the desktop's own snapshot, else `session · pane`. */
function paneLabel(session: string, target: string | null): string {
  if (!target) return session
  const { sessions, runtimesBySession } = useHerdrStore.getState()
  const local = sessions.find(item => item.name === session && (!item.hostId || item.hostId === LOCAL_HOST_ID))
  const agent = runtimesBySession[sessionScope(local) ?? session]?.snapshot?.agents.find(item => item.paneId === target)
  return agent?.name ?? `${session} · ${target}`
}

/** Phone actions are not the desktop user's: kind `audit`, owned by no workspace. */
function log(level: "info" | "warn", event: string, message: string, metadata: Record<string, unknown>) {
  void invoke("log_event", {
    event: { level, kind: "audit", source: "companion", workspace_path: null, event, message, metadata },
  }).catch(() => undefined)
}

/**
 * Records what phones do and tells the desktop user: every audit goes to
 * Logs, writes and control show a notice, and a takeover also notifies the
 * system while Yuzora is in the background.
 */
export function reportCompanionEvent(event: CompanionEvent): void {
  switch (event.type) {
    case "audit": {
      const device = deviceLabel(event.deviceId)
      const pane = paneLabel(event.session, event.target)
      const summary = Object.fromEntries(SUMMARY_FIELDS.filter(key => key in event.summary).map(key => [key, event.summary[key]]))
      const stalled = event.action === "herdr-io-stalled"
      log(stalled ? "warn" : "info", "companion_audit", `${device}: ${event.action} on ${pane}`, {
        deviceId: event.deviceId, peerId: event.peerId, scope: event.scope, action: event.action,
        session: event.session, target: event.target, reason: event.reason, outcome: event.outcome, at: event.at, summary,
      })
      if (WRITES.has(event.action)) {
        toast(i18n.t(`companion:audit.write.${event.action}`, { device, pane }), { id: `companion-${event.deviceId}-${event.target}` })
      } else if (stalled) {
        toast.warning(i18n.t("companion:audit.stalled", { device, pane }))
      }
      return
    }
    case "control.state": {
      // Opening is also audited as `terminal.control`; only the state shows a notice.
      if (!event.open) return
      const title = i18n.t(event.takeover ? "companion:audit.takeover" : "companion:audit.control", {
        device: deviceLabel(event.deviceId),
        pane: paneLabel(event.session, event.target),
      })
      const id = `companion-control-${event.stream}`
      if (!event.takeover) {
        toast(title, { id })
        return
      }
      toast.warning(title, { id })
      if (!document.hasFocus()) {
        void isPermissionGranted().then(granted => {
          if (granted) sendNotification({ title })
        }).catch(error => console.warn("Companion notification delivery failed", error))
      }
      return
    }
    case "audit.dropped":
      log("warn", "companion_audit_dropped", `${event.lost} Companion audit events lost`, {
        lost: event.lost, lostTightening: event.lostTightening, rejectedOmitted: event.rejectedOmitted, since: event.since,
      })
      if (event.lostTightening > 0) toast.warning(i18n.t("companion:audit.droppedTightening", { count: event.lostTightening }))
      else if (event.lost > 0) toast.warning(i18n.t("companion:audit.dropped", { count: event.lost }))
      return
  }
}
