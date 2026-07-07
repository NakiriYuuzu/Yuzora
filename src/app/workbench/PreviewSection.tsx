import { useState } from "react"
import { useTranslation } from "react-i18next"

import { SettingCard, SettingsTextInput } from "./settingsPrimitives"
import {
  PREVIEW_SETTINGS_STORAGE_KEY,
  loadPreviewSettings,
  writeJsonSetting,
  type PreviewSettings,
} from "./settingsStorage"

export function PreviewSection() {
  const { t } = useTranslation("preview")
  const [settings, setSettings] = useState(loadPreviewSettings)

  const update = (patch: Partial<PreviewSettings>) => {
    const next = { ...settings, ...patch }
    setSettings(next)
    writeJsonSetting(PREVIEW_SETTINGS_STORAGE_KEY, next)
  }

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard
        label={t("devServerLabel")}
        sub={t("devServerDescription")}
      >
        <div className="flex flex-col gap-[12px]">
          <SettingsTextInput
            label="Dev server command override"
            value={settings.command}
            placeholder="bun run dev"
            onChange={(command) => update({ command })}
          />
          <SettingsTextInput
            label="Port override"
            type="number"
            value={settings.port}
            placeholder="5173"
            onChange={(port) => update({ port })}
          />
        </div>
      </SettingCard>
    </div>
  )
}
