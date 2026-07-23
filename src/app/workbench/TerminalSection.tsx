import { useTranslation } from "react-i18next"

import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  type TerminalSettings,
} from "@/app/workbench/settingsStorage"
import { isWindowsPlatform } from "@/lib/platform"
import type { TerminalProfile } from "@/lib/types"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import {
  EMPTY_CUSTOM_TERMINAL_PROFILE,
  availableTerminalProfiles,
  terminalProfileDisplayName,
} from "@/terminal/terminalProfiles"
import { useTerminalProfiles } from "@/terminal/useTerminalProfiles"

import { Segmented, SettingCard, SettingsTextInput } from "./settingsPrimitives"
export function TerminalSection() {
  const { t } = useTranslation("terminal")
  const settings = useTerminalSettingsStore()
  const updateSettings = useTerminalSettingsStore((state) => state.update)
  const discoveredProfiles = useTerminalProfiles()
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const terminalRatioScope = useWorkbenchLayoutStore((state) => state.terminalRatioScope)
  const setTerminalRatioScope = useWorkbenchLayoutStore((state) => state.setTerminalRatioScope)

  const update = (patch: Partial<TerminalSettings>) => {
    updateSettings(patch)
  }
  const selectProfile = (profile: TerminalProfile) => {
    update({ defaultProfile: profile })
  }
  const updateCustomProfile = (patch: Partial<TerminalProfile>) => {
    const customProfile = {
      ...settings.customProfile,
      ...patch,
      id: "custom",
      name: t("customProfileName"),
      kind: "custom" as const,
    }
    update({
      customProfile,
      ...(settings.defaultProfile.id === "custom"
        ? { defaultProfile: customProfile }
        : {}),
    })
  }
  const selectableProfiles = availableTerminalProfiles(
    discoveredProfiles,
    settings.defaultProfile,
    settings.customProfile,
  )
  if (!selectableProfiles.some((profile) => profile.id === "custom")) {
    selectableProfiles.push({
      ...EMPTY_CUSTOM_TERMINAL_PROFILE,
      name: t("customProfileName"),
    })
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard
        label={t("sizeMemoryLabel")}
        sub={t("sizeMemoryDescription")}
      >
        <Segmented
          label={t("sizeMemoryLabel")}
          options={[
            { id: "global", label: t("sizeMemoryGlobal") },
            { id: "workspace", label: t("sizeMemoryWorkspace") },
          ]}
          value={terminalRatioScope}
          onChange={(scope) => {
            if (scope === "global" || scope === "workspace") {
              setTerminalRatioScope(scope, workspacePath)
            }
          }}
        />
      </SettingCard>

      <SettingCard
        label={t("fontSizeLabel")}
        sub={t("fontSizeDescription")}
      >
        <label className="flex items-center gap-[10px]">
          <input
            id="terminal-font-size"
            type="range"
            min={MIN_TERMINAL_FONT_SIZE}
            max={MAX_TERMINAL_FONT_SIZE}
            step={1}
            value={settings.fontSize}
            aria-label={t("fontSizeLabel")}
            onChange={(event) => update({ fontSize: Number(event.currentTarget.value) })}
            className="min-w-0 flex-1 accent-(--yz-accent)"
          />
          <output
            htmlFor="terminal-font-size"
            className="min-w-[42px] text-right font-mono text-[11.5px] text-(--ink-2)"
          >
            {t("fontSizeValue", { size: settings.fontSize })}
          </output>
        </label>
      </SettingCard>

      <SettingCard
        label={t("shellLabel")}
        sub={t("shellDescription")}
      >
        <div className="flex flex-col gap-[12px]">
          <label className="flex flex-col gap-[6px]">
            <span className="text-[11.5px] font-medium text-(--ink-2)">
              {t("defaultProfileLabel")}
            </span>
            <select
              aria-label={t("defaultProfileLabel")}
              value={settings.defaultProfile.id}
              onChange={(event) => {
                const profile = selectableProfiles.find(
                  (candidate) => candidate.id === event.currentTarget.value,
                )
                if (profile) selectProfile(profile)
              }}
              className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none focus:border-(--yz-accent)"
            >
              {selectableProfiles.map((profile) => (
                <option key={profile.id} value={profile.id}>
                  {terminalProfileDisplayName(profile)}
                </option>
              ))}
            </select>
          </label>
          <SettingsTextInput
            label={t("customExecutableLabel")}
            value={settings.customProfile.shell}
            placeholder="C:\Program Files\PowerShell\7\pwsh.exe"
            onChange={(shell) => updateCustomProfile({ shell })}
          />
          <label className="flex flex-col gap-[6px]">
            <span className="text-[11.5px] font-medium text-(--ink-2)">
              {t("customArgsLabel")}
            </span>
            <textarea
              aria-label={t("customArgsLabel")}
              rows={3}
              value={settings.customProfile.args.join("\n")}
              placeholder={"-NoLogo\n-NoProfile"}
              onChange={(event) => {
                const args = event.currentTarget.value
                  .split("\n")
                  .map((arg) => arg.trim())
                  .filter(Boolean)
                updateCustomProfile({ args })
              }}
              className="resize-y rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] py-[7px] font-mono text-[11.5px] text-(--ink-1) outline-none transition-colors placeholder:text-(--ink-4) focus:border-(--yz-accent)"
            />
            <span className="text-[10.5px] text-(--ink-3)">{t("customArgsHint")}</span>
          </label>
          <Segmented
            label={t("customCwdStrategyLabel")}
            options={[
              { id: "native", label: t("customCwdNative") },
              { id: "wsl", label: t("customCwdWsl") },
            ]}
            value={settings.customProfile.cwdStrategy}
            onChange={(cwdStrategy) => {
              if (cwdStrategy === "native" || cwdStrategy === "wsl") {
                updateCustomProfile({ cwdStrategy })
              }
            }}
          />
        </div>
      </SettingCard>

      {isWindowsPlatform() && (
        <SettingCard label={t("imeLabel")} sub={t("imeDescription")}>
          <Segmented
            label={t("imeLabel")}
            options={[
              { id: "cursor", label: t("imeCursorAnchor") },
              { id: "tui", label: t("imeTuiAnchor") },
            ]}
            value={settings.imeAnchorMode}
            onChange={(mode) => {
              if (mode === "cursor" || mode === "tui") update({ imeAnchorMode: mode })
            }}
          />
        </SettingCard>
      )}
    </div>
  )
}
