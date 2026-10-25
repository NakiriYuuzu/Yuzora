import { describe, expect, it } from "vitest"
import en from "./locales/en.json"
import zhTW from "./locales/zh-TW.json"
import { i18n, pickLanguage } from "./i18n"

function keys(value: unknown, prefix = ""): string[] {
  if (typeof value !== "object" || value === null) return [prefix]
  return Object.entries(value).flatMap(([key, child]) => keys(child, prefix ? `${prefix}.${key}` : key))
}

describe("Companion i18n", () => {
  it("fills every key in both languages", () => {
    expect(keys(zhTW).sort()).toEqual(keys(en).sort())
    for (const locale of [en, zhTW]) for (const key of keys(locale)) expect(key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], locale), key).not.toBe("")
  })

  it("picks Traditional Chinese for any Chinese preference and English otherwise", () => {
    expect(pickLanguage(["zh-TW"])).toBe("zh-TW")
    expect(pickLanguage(["zh-Hant-HK", "en"])).toBe("zh-TW")
    expect(pickLanguage(["en-US", "zh-TW"])).toBe("en")
    expect(pickLanguage(["ja-JP"])).toBe("en")
    expect(pickLanguage([])).toBe("en")
  })

  it("keeps the document language on the language it renders", async () => {
    expect(document.documentElement.lang).toBe(i18n.language)
    await i18n.changeLanguage("zh-TW")
    expect(document.documentElement.lang).toBe("zh-TW")
    await i18n.changeLanguage("en")
    expect(document.documentElement.lang).toBe("en")
  })
})
