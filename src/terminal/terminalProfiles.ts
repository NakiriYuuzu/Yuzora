import i18n from "@/lib/i18n"
import type { TerminalProfile } from "@/lib/types"

export const SYSTEM_TERMINAL_PROFILE: TerminalProfile = {
    id: "system",
    name: "System default",
    shell: "",
    args: [],
    kind: "system",
    cwdStrategy: "native"
}

export const EMPTY_CUSTOM_TERMINAL_PROFILE: TerminalProfile = {
    id: "custom",
    name: "Custom",
    shell: "",
    args: [],
    kind: "custom",
    cwdStrategy: "native"
}

export function availableTerminalProfiles(
    discovered: TerminalProfile[],
    defaultProfile: TerminalProfile,
    customProfile: TerminalProfile
): TerminalProfile[] {
    const candidates = [
        SYSTEM_TERMINAL_PROFILE,
        ...discovered,
        defaultProfile,
        customProfile
    ]
    const profiles = new Map<string, TerminalProfile>()
    for (const profile of candidates) {
        if (profile.id === "custom" && profile.shell.trim().length === 0) continue
        if (!profiles.has(profile.id)) profiles.set(profile.id, profile)
    }
    return [...profiles.values()]
}

export function terminalProfileDisplayName(profile: TerminalProfile): string {
    if (profile.id === "system") return i18n.t("profileSystem", { ns: "terminal" })
    if (profile.id === "custom") return i18n.t("profileCustom", { ns: "terminal" })
    if (profile.id === "cmd") return i18n.t("profileCmd", { ns: "terminal" })
    if (profile.id === "windows-powershell") {
        return i18n.t("profileWindowsPowerShell", { ns: "terminal" })
    }
    if (profile.id === "powershell-7") {
        return i18n.t("profilePowerShell7", { ns: "terminal" })
    }
    if (profile.id === "wsl") return i18n.t("profileWslDefault", { ns: "terminal" })
    if (profile.kind === "wsl") {
        const distroFlag = profile.args.findIndex(
            (arg) => arg === "--distribution" || arg === "-d"
        )
        const distro = profile.args[distroFlag + 1] ?? profile.name
        return i18n.t("profileWslDistro", { ns: "terminal", distro })
    }
    return profile.name
}
