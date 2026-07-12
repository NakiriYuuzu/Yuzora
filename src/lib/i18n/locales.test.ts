import { describe, expect, it } from "vitest"

type LocaleTree = Record<string, unknown>

const enModules = import.meta.glob<LocaleTree>("./locales/en/**/*.json", {
  eager: true,
  import: "default"
})
const zhTwModules = import.meta.glob<LocaleTree>("./locales/zh-TW/**/*.json", {
  eager: true,
  import: "default"
})

function relativeNamespace(path: string, locale: "en" | "zh-TW"): string {
  return path.replace(`./locales/${locale}/`, "")
}

function leafKeys(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return [prefix]
  return Object.entries(value).flatMap(([key, child]) =>
    leafKeys(child, prefix ? `${prefix}.${key}` : key)
  )
}

function normalizedKeys(value: LocaleTree): string[] {
  return [...new Set(leafKeys(value).map((key) =>
    key.replace(/_(zero|one|two|few|many|other)$/, "_plural")
  ))].sort()
}

describe("locale namespace parity", () => {
  const enByNamespace = Object.fromEntries(Object.entries(enModules).map(([path, value]) => [
    relativeNamespace(path, "en"),
    value
  ]))
  const zhTwByNamespace = Object.fromEntries(Object.entries(zhTwModules).map(([path, value]) => [
    relativeNamespace(path, "zh-TW"),
    value
  ]))

  it("keeps the English and Traditional Chinese namespace inventory aligned", () => {
    expect(Object.keys(zhTwByNamespace).sort()).toEqual(Object.keys(enByNamespace).sort())
  })

  it.each(Object.keys(enByNamespace).sort())(
    "keeps normalized nested keys aligned for %s",
    (namespace) => {
      expect(normalizedKeys(zhTwByNamespace[namespace])).toEqual(
        normalizedKeys(enByNamespace[namespace])
      )
    }
  )

  it.each(["panels.json", "workbench.json"])(
    "keeps exact product keys aligned for %s",
    (namespace) => {
      expect(leafKeys(zhTwByNamespace[namespace]).sort()).toEqual(
        leafKeys(enByNamespace[namespace]).sort()
      )
    }
  )
})
