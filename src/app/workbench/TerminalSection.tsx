import { useTranslation } from "react-i18next"

import {
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  type TerminalSettings,
} from "@/app/workbench/settingsStorage"
import { isWindowsPlatform } from "@/lib/platform"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { TERMINAL_FONTS, normalizeTerminalFontFamily, terminalFontStack } from "@/terminal/terminalFonts"

import {Slider} from '@/components/ui/slider'
import {Select,SelectContent,SelectGroup,SelectItem,SelectTrigger,SelectValue} from '@/components/ui/select'
import {Field,FieldGroup,FieldLabel} from '@/components/ui/field'
import { Segmented, SettingCard } from "./settingsPrimitives"
export function TerminalSection() {
  const { t } = useTranslation("terminal")
  const settings = useTerminalSettingsStore()
  const updateSettings = useTerminalSettingsStore((state) => state.update)

  const update = (patch: Partial<TerminalSettings>) => {
    updateSettings(patch)
  }

  return (
    <FieldGroup className="settings-fields">
      <SettingCard label={t("typographyLabel")} sub={t("fontFamilyDescription")}>
        <FieldGroup className="settings-terminal-typography">
        <Field data-settings-label={t("fontFamilyLabel")}><FieldLabel htmlFor="settings-terminal-font">{t("fontFamilyLabel")}</FieldLabel>
        <Select value={settings.fontFamily} onValueChange={value=>update({fontFamily:normalizeTerminalFontFamily(value)})}>
          <SelectTrigger id="settings-terminal-font" className="settings-terminal-font-select"><SelectValue/></SelectTrigger>
          <SelectContent><SelectGroup>{TERMINAL_FONTS.map(font=><SelectItem key={font.id} value={font.id}>{font.id==='system'?t("systemMonospace"):font.name}{font.id==='jetbrains'?` · ${t("defaultFont")}`:''}</SelectItem>)}</SelectGroup></SelectContent>
        </Select>
        </Field>
        <Field data-settings-label={t("fontSizeLabel")}><FieldLabel>{t("fontSizeLabel")}</FieldLabel>
        <div className="settings-slider-row">
          <Slider min={MIN_TERMINAL_FONT_SIZE} max={MAX_TERMINAL_FONT_SIZE} step={1} value={[settings.fontSize]} aria-label={t("fontSizeLabel")} onValueChange={value=>update({fontSize:value[0]})}/>
          <output className="settings-slider-value">{t("fontSizeValue",{size:settings.fontSize})}</output>
        </div>
        </Field>
        </FieldGroup>
        <p className="settings-inline-hint">{t("fontFallbackHint")}</p>
      </SettingCard>

      <div className="settings-terminal-preview" role="img" aria-label={t("fontPreview")} data-design="replica-terminal-font-preview" data-design-label={t("fontPreview")}>
        <div><span>{t("fontPreview")}</span><span>{settings.fontSize} px</span></div>
        <pre style={{fontFamily:terminalFontStack(settings.fontFamily),fontSize:settings.fontSize}}><span>~/yuzora</span>{'  main\n$ '}<span>echo "Hello, Yuzora"</span>{'\nHello, Yuzora\n'}{t("fontPreviewSample")}{'\n0O  1lI  {} [] ()  => != / \\ _'}</pre>
      </div>

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
    </FieldGroup>
  )
}
