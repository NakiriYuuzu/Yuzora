import { getVersion } from "@tauri-apps/api/app"
import { check, Update } from "@tauri-apps/plugin-updater"
import { invoke } from "./ipc"

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
    if (channel === "stable") return check({ timeout: 20_000 })
    const metadata = await invoke<ConstructorParameters<typeof Update>[0] | null>("check_preview_update")
    return metadata ? new Update(metadata) : null
}
