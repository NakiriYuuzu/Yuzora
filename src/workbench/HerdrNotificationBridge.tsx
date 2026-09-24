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
import i18n from "@/lib/i18n"

export function HerdrNotificationBridge() {
  useEffect(() => {
    let disposed = false
    const unsubscribe = useHerdrStore.subscribe((state, previous) => {
      if (state.attentionByKey === previous.attentionByKey) return
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
        if (settings.system) void isPermissionGranted().then(granted => {
          const latest = useHerdrStore.getState().attentionByKey.get(item.key)
          if (!disposed && granted && useHerdrNotificationStore.getState().system && latest?.kind === item.kind && latest.updatedAt === item.updatedAt && !latest.seen) sendNotification({ title, body: description })
        }).catch(error => console.warn("HERDR notification delivery failed", error))
        if (settings.sound) void playHerdrSound(item.kind as "done" | "blocked").catch(error => console.warn("HERDR notification sound failed", error))
      }
    })
    return () => { disposed = true; unsubscribe() }
  }, [])
  return null
}
