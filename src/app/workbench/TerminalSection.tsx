import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

import { Segmented, SettingCard, SettingsTextInput } from "./settingsPrimitives"
import {
  TERMINAL_SETTINGS_STORAGE_KEY,
  loadTerminalSettings,
  writeJsonSetting,
  type TerminalSettings,
} from "./settingsStorage"

export function TerminalSection() {
  const { t } = useTranslation("terminal")
  const [settings, setSettings] = useState(loadTerminalSettings)
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const terminalRatioScope = useWorkbenchLayoutStore((state) => state.terminalRatioScope)
  const setTerminalRatioScope = useWorkbenchLayoutStore((state) => state.setTerminalRatioScope)

  const update = (patch: Partial<TerminalSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, next)
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
        label={t("shellLabel")}
        sub={t("shellDescription")}
      >
        <div className="flex flex-col gap-[12px]">
          <SettingsTextInput
            label="Shell path override"
            value={settings.shellPath}
            placeholder="/opt/homebrew/bin/fish"
            onChange={(shellPath) => update({ shellPath })}
          />
          <SettingsTextInput
            label="Default shell args"
            value={settings.shellArgs}
            placeholder="-l"
            onChange={(shellArgs) => update({ shellArgs })}
          />
        </div>
      </SettingCard>
    </div>
  )
}
