import { getVersion } from "@tauri-apps/api/app"
import { check, Update } from "@tauri-apps/plugin-updater"
import { invoke } from "./ipc"
import { isTauri } from "./platform"

export type UpdateChannel = "auto" | "stable" | "preview"
export const UPDATE_CHANNEL_KEY = "yuzora.update.channel.v1"

export function loadUpdateChannel(): UpdateChannel {
    try {
        const value = localStorage.getItem(UPDATE_CHANNEL_KEY)
        return value === "stable" || value === "preview" ? value : "auto"
    } catch { return "auto" }
}

export function resolveUpdateChannel(preference: UpdateChannel, version: string): "stable" | "preview" {
    if (preference !== "auto") return preference
    return /^\d+\.\d+\.\d+-[0-9A-Za-z.-]+(?:\+[0-9A-Za-z.-]+)?$/.test(version) ? "preview" : "stable"
}

export async function checkChannelUpdate(preference: UpdateChannel): Promise<Update | null> {
    const channel = resolveUpdateChannel(preference, await getVersion())
    if (!isTauri()) return check({ timeout: 20_000 })
    let metadata: ConstructorParameters<typeof Update>[0] | null | undefined
    try {
        metadata = await invoke<ConstructorParameters<typeof Update>[0] | null>("check_release_update", {
            includePreview: channel === "preview",
        })
    } catch (error) {
        const message = String(error).toLowerCase()
        const commandUnavailable = message.includes("command") &&
            (message.includes("not found") || message.includes("unknown") || message.includes("unrecognized"))
        if (!commandUnavailable) throw error
    }
    // Keep development shells and older runtimes usable while the native
    // command is being rolled out. A real Tauri command always returns null or
    // metadata; undefined means the command is unavailable in the host.
    if (typeof metadata === "undefined") return check({ timeout: 20_000 })
    return metadata ? new Update(metadata) : null
}
