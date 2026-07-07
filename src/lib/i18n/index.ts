/**
 * i18n framework (i18next + react-i18next). Initializes the shared i18next
 * singleton synchronously on import; import it once at app entry (main.tsx)
 * before rendering, and from the vitest setup so t()/useTranslation are ready.
 *
 * ── Key naming convention (read before adding strings) ───────────────────────
 * • Namespace-per-domain, one file per namespace per locale:
 *     src/lib/i18n/locales/{en,zh-TW}/<ns>.json
 *   Namespaces are auto-discovered from those files (import.meta.glob below), so
 *   dropping a new <ns>.json into BOTH locale folders registers it with no edit
 *   here. Keep each domain in its own ns file so parallel migrations never
 *   collide on a single shared file.
 * • `common` is the default namespace (shared / app-wide copy). Feature areas
 *   get their own ns: `lsp`, `git`, `terminal`, `preview`, …
 * • Keys are camelCase. Nest one level where it groups naturally
 *   (e.g. common `language.system`); the leaf value is the user-facing string.
 * • Interpolation uses i18next `{{name}}` placeholders, e.g.
 *     "greeting": "Hello {{name}}"   →   t("greeting", { name })
 * • Access: React → useTranslation("<ns>") then t("key"). Non-React → import
 *   this module's default export and call i18n.t("key", { ns: "<ns>" }).
 * ─────────────────────────────────────────────────────────────────────────────
 */
import i18n from "i18next"
import type { Resource, ResourceLanguage } from "i18next"
import { initReactI18next } from "react-i18next"

export type LanguagePreference = "system" | "en" | "zh-TW"

// Eagerly import every locales/<lng>/<ns>.json so init needs no async backend
// (synchronous init → deterministic tests, usable t() the moment this imports).
const modules = import.meta.glob("./locales/*/*.json", {
    eager: true,
    import: "default"
}) as Record<string, ResourceLanguage>

const resources: Resource = {}
for (const [filePath, content] of Object.entries(modules)) {
    const match = filePath.match(/\.\/locales\/([^/]+)\/([^/]+)\.json$/)
    if (!match) continue
    const [, lng, ns] = match
    const bundle = (resources[lng] ??= {})
    bundle[ns] = content
}

// Namespaces derived from the loaded files, so a new <ns>.json self-registers.
const namespaces = [...new Set(Object.values(resources).flatMap((byNs) => Object.keys(byNs)))]

export const LANGUAGE_STORAGE_KEY = "yuzora:language"

// Persisted user choice: "system" (or absent) follows the OS; "en"/"zh-TW" pin it.
export function getLanguagePreference(): LanguagePreference {
    try {
        const raw = localStorage.getItem(LANGUAGE_STORAGE_KEY)
        if (raw === "en" || raw === "zh-TW" || raw === "system") return raw
    } catch {
        /* private mode / unavailable storage — fall through to system */
    }
    return "system"
}

// Resolve a preference to a concrete locale: "system" maps any zh* navigator
// language to zh-TW, everything else to the "en" fallback.
export function resolveLanguage(pref: LanguagePreference): "en" | "zh-TW" {
    if (pref === "en" || pref === "zh-TW") return pref
    const nav = typeof navigator !== "undefined" ? navigator.language : ""
    return nav?.toLowerCase().startsWith("zh") ? "zh-TW" : "en"
}

// Persist a preference and switch the live language immediately.
export function setLanguagePreference(pref: LanguagePreference): void {
    try {
        localStorage.setItem(LANGUAGE_STORAGE_KEY, pref)
    } catch {
        /* keep the in-memory language change even if persistence fails */
    }
    void i18n.changeLanguage(resolveLanguage(pref))
}

void i18n.use(initReactI18next).init({
    resources,
    lng: resolveLanguage(getLanguagePreference()),
    fallbackLng: "en",
    ns: namespaces,
    defaultNS: "common",
    // Bundled resources + no backend → init synchronously so t()/useTranslation
    // work on import (non-React callers, deterministic test setup).
    initAsync: false,
    interpolation: { escapeValue: false }
})

export default i18n
