import { showActionError } from "@/lib/actionFeedback"
import i18n from "@/lib/i18n"
import { toast } from "sonner"
import { useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useUiStore } from "@/state/uiStore"
import { openCreatedHerdrTabAndRequestName } from "@/lib/herdrTabActions"

/** Create through the selected Host Runtime; never create an unrelated local PTY. */
export async function openNewTerminalTab(groupIndex = useWorkspaceStore.getState().activeGroupIndex) {
    const runtime = useHerdrStore.getState()
    if (!runtime.canCreateTerminal()) {
        toast.info(i18n.t("terminalUnavailableTitle", { ns: "workTabs" }), { id: "yuzora-terminal-unavailable", description: i18n.t("terminalUnavailableDescription", { ns: "workTabs" }) })
        return false
    }
    try {
    const created = await runtime.createTerminalInSelectedSpace()
    if (!created) return false
    useUiStore.getState().setMode("files")
    await openCreatedHerdrTabAndRequestName({
        sessionName: created.herdrSessionId,
        workspaceId: created.workspaceId,
        terminalId: created.terminalId,
        title: created.title,
        paneId: created.paneId,
        tabId: created.tabId,
        groupIndex
    })
    return true
    } catch (error) {
        await showActionError(i18n.t("newTerminal", { ns: "workTabs" }), error)
        return false
    }
}
