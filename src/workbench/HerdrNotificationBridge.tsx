import { useEffect } from "react"
import { toast } from "sonner"
import { isPermissionGranted, sendNotification } from "@tauri-apps/plugin-notification"
import { useHerdrStore } from "@/state/herdrStore"
import { useHerdrNotificationStore } from "@/state/herdrNotificationStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { herdrPageMatchesSnapshotSession } from "@/lib/workbenchTabReorder"
import { localDefaultScope } from "@/lib/herdrProvider"
import { newHerdrAttention, playHerdrSound } from "@/lib/herdrNotifications"
import type { HerdrAttentionItem } from "@/lib/herdrTypes"
import i18n from "@/lib/i18n"

export function HerdrNotificationBridge() {
  useEffect(() => {
    let disposed = false
    const pending = new Map<string, HerdrAttentionItem>()
    const unsubscribe = useHerdrStore.subscribe((state, previous) => {
      if (state.attentionByKey === previous.attentionByKey) return
      for (const [key, item] of pending) {
        const latest = state.attentionByKey.get(key)
        if (!latest || latest.kind !== item.kind || latest.seen) pending.delete(key)
      }
      const settings = useHerdrNotificationStore.getState()
      for (const item of newHerdrAttention(previous.attentionByKey, state.attentionByKey)) {
        // Initial snapshots and reconnects populate history; they are not new work.
        const previousRuntime = previous.runtimesBySession[item.sessionName]
        const runtime = state.runtimesBySession[item.sessionName]
        if (previousRuntime?.connectionState !== "ready" || runtime?.connectionState !== "ready") continue
        // applySnapshot updates baseSnapshot and attention atomically even when
        // a dropped subscription leaves the runtime's connection state ready.
        if (previousRuntime.baseSnapshot !== runtime.baseSnapshot) continue
        if ((item.kind === "done" && !settings.done) || (item.kind === "blocked" && !settings.blocked)) continue
        const agent = runtime?.snapshot?.agents.find(agent => agent.paneId === item.paneId)
        const workspace = useWorkspaceStore.getState()
        const activeGroup = workspace.groups[workspace.activeGroupIndex]
        const activeTab = activeGroup?.tabs.find(tab => tab.path === activeGroup.activePath)
        const defaultScope = localDefaultScope(state.sessions)
        if (document.hasFocus() && (
          (useHerdrNativeStore.getState().selection?.sessionName === item.sessionName && runtime.snapshot?.focusedPaneId === item.paneId)
          || (activeTab?.kind === "herdr-terminal" && herdrPageMatchesSnapshotSession(activeTab.herdrSessionId, item.sessionName, defaultScope)
            && ((agent?.tabId && activeTab.herdrTabId === agent.tabId) || activeTab.paneId === item.paneId))
        )) continue
        const title = i18n.t(item.kind === "done" ? "herdrTools:agentDone" : "herdrTools:agentBlocked", { name: item.title ?? item.displayAgent ?? item.paneId })
        const description = `${item.sessionName} · ${item.workspaceId ?? ""}`
        if (settings.toast) toast(title, { id: item.key, description, action: agent ? { label: i18n.t("herdrTools:open"), onClick: () => { void useHerdrStore.getState().activateAgent({ ...agent, sessionName: item.sessionName }) } } : undefined })
        if (settings.system) {
          // Same-kind refreshes belong to this transition; a later turn gets
          // a different item even when the clock timestamp is identical.
          pending.set(item.key, item)
          void isPermissionGranted().then(granted => {
            const current = useHerdrStore.getState()
            const latest = current.attentionByKey.get(item.key)
            const latestSettings = useHerdrNotificationStore.getState()
            const enabled = item.kind === "done" ? latestSettings.done : latestSettings.blocked
            if (!disposed && granted && latestSettings.system && enabled
              && pending.get(item.key) === item && latest?.kind === item.kind && !latest.seen) sendNotification({ title, body: description })
          }).catch(error => console.warn("HERDR notification delivery failed", error))
            .finally(() => { if (pending.get(item.key) === item) pending.delete(item.key) })
        }
        if (settings.sound) void playHerdrSound(item.kind as "done" | "blocked").catch(error => console.warn("HERDR notification sound failed", error))
      }
    })
    return () => { disposed = true; pending.clear(); unsubscribe() }
  }, [])
  return null
}
