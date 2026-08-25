import i18n from "@/lib/i18n"
import { herdrTabClose, herdrTabRename } from "@/lib/herdrIpc"
import { useHerdrStore } from "@/state/herdrStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface RenameHerdrTabOptions {
    sessionName: string
    tabId: string
    currentLabel: string
    pagePath?: string | null
}

interface OpenCreatedHerdrTabOptions {
    sessionName: string
    workspaceId?: string | null
    terminalId: string
    paneId?: string | null
    tabId?: string | null
    title?: string | null
    groupIndex?: number
}

function errorMessage(error: unknown): string | null {
    if (typeof error === "string") return error.trim()
    if (error instanceof Error) return error.message.trim()
    return null
}

export function isHerdrTabAlreadyClosedError(
    error: unknown,
    tabId: string
): boolean {
    const match = errorMessage(error)?.match(
        /^tab_not_found:\s*tab\s+(\S+)\s+not found$/
    )
    return match?.[1] === tabId
}

/**
 * Close remains runtime-first. A precisely matched tab_not_found response is
 * idempotent success because the requested runtime tab is already gone.
 */
export async function closeHerdrTabIdempotently(
    sessionName: string,
    tabId: string
): Promise<void> {
    try {
        await herdrTabClose({ sessionName, tabId })
    } catch (error) {
        if (!isHerdrTabAlreadyClosedError(error, tabId)) throw error
    }
}

async function requestHerdrTabName(initialValue: string): Promise<string | null> {
    return useTextInputDialogStore.getState().request({
        title: i18n.t("textInputDialog.herdrTabTitle", { ns: "menus" }),
        description: i18n.t("textInputDialog.herdrTabDescription", { ns: "menus" }),
        label: i18n.t("textInputDialog.nameLabel", { ns: "menus" }),
        initialValue,
        confirmLabel: i18n.t("textInputDialog.rename", { ns: "menus" })
    })
}

async function reconcileHerdrTabMutation(sessionName: string): Promise<void> {
    useHerdrStore.getState().bumpTopologyRevision()
    await useHerdrStore.getState().refreshSnapshot(sessionName).catch(() => undefined)
}

export async function renameHerdrTabWithDialog({
    sessionName,
    tabId,
    currentLabel,
    pagePath
}: RenameHerdrTabOptions): Promise<boolean> {
    const label = await requestHerdrTabName(currentLabel)
    if (!label || label === currentLabel) return false

    await herdrTabRename({ sessionName, tabId, label })
    if (pagePath) {
        useWorkspaceStore.getState().updateHerdrPageTitle(pagePath, label)
    }
    await reconcileHerdrTabMutation(sessionName)
    return true
}

export async function openCreatedHerdrTab({
    sessionName,
    workspaceId,
    terminalId,
    paneId,
    tabId,
    title,
    groupIndex
}: OpenCreatedHerdrTabOptions): Promise<void> {
    const initialTitle = title?.trim() || terminalId
    useWorkspaceStore.getState().openHerdrTerminalPage({
        herdrSessionId: sessionName,
        terminalId,
        title: initialTitle,
        paneId: paneId ?? null,
        herdrTabId: tabId ?? null,
        herdrWorkspaceId: workspaceId ?? null,
        groupIndex
    })
}

export async function openCreatedHerdrTabAndRequestName({
    sessionName,
    workspaceId,
    terminalId,
    paneId,
    tabId,
    title,
    groupIndex
}: OpenCreatedHerdrTabOptions): Promise<void> {
    const initialTitle = title?.trim() || terminalId
    await openCreatedHerdrTab({
        sessionName,
        workspaceId,
        terminalId,
        paneId,
        tabId,
        title: initialTitle,
        groupIndex
    })

    if (!tabId) return
    const page = useWorkspaceStore
        .getState()
        .groups.flatMap((group) => group.tabs)
        .find(
            (candidate) =>
                candidate.kind === "herdr-terminal" &&
                candidate.herdrSessionId === sessionName &&
                candidate.herdrTabId === tabId
        )
    await renameHerdrTabWithDialog({
        sessionName,
        tabId,
        currentLabel: initialTitle,
        pagePath: page?.path ?? null
    })
}
