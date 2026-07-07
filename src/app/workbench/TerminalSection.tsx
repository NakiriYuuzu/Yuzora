import { useState } from "react"
import { useTranslation } from "react-i18next"

import { SettingCard, SettingsTextInput } from "./settingsPrimitives"
import {
  TERMINAL_SETTINGS_STORAGE_KEY,
  loadTerminalSettings,
  writeJsonSetting,
  type TerminalSettings,
} from "./settingsStorage"

export function TerminalSection() {
  const { t } = useTranslation("terminal")
  const [settings, setSettings] = useState(loadTerminalSettings)

  const update = (patch: Partial<TerminalSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, next)
  }

  return (
    <div className="flex flex-col gap-[14px]">
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
