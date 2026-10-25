import { createInstance } from "i18next"
import en from "./locales/en.json"
import zhTW from "./locales/zh-TW.json"

export function pickLanguage(languages: readonly string[]): "en" | "zh-TW" {
  for (const language of languages) {
    if (/^zh\b/i.test(language)) return "zh-TW"
    if (/^en\b/i.test(language)) return "en"
  }
  return "en"
}

/** The PWA's own instance, separate from the desktop's i18n singleton; App provides it. */
export const i18n = createInstance()
void i18n.init({
  resources: { en: { translation: en }, "zh-TW": { translation: zhTW } },
  lng: pickLanguage(typeof navigator === "undefined" ? [] : navigator.languages),
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  initAsync: false,
})

// The static shell says `en`; the page follows the language it renders.
const setDocumentLang = (language: string) => {
  if (typeof document !== "undefined") document.documentElement.lang = language
}
setDocumentLang(i18n.language)
i18n.on("languageChanged", setDocumentLang)
