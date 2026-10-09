import { describe, expect, it } from "vitest"
import en from "@/lib/i18n/locales/en/machines.json"
import zh from "@/lib/i18n/locales/zh-TW/machines.json"
import { describeMachineError, MACHINE_ERROR_CODES, parseMachineError } from "./machinesErrors"

function leaves(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix]
  return Object.entries(value).flatMap(([key, child]) => leaves(child, prefix ? `${prefix}.${key}` : key))
}

describe("machines i18n", () => {
  it("keeps en and zh-TW key sets identical", () => {
    expect(leaves(zh).sort()).toEqual(leaves(en).sort())
  })
  it.each(["en", "zh-TW"] as const)("%s has a message for every contract error code and the fallback", (locale) => {
    const errors = (locale === "en" ? en : zh).errors as Record<string, string>
    for (const code of MACHINE_ERROR_CODES) expect(errors[code], code).toBeTruthy()
    expect(errors.unknown).toBeTruthy()
    expect(MACHINE_ERROR_CODES).toHaveLength(22)
  })
})

describe("describeMachineError", () => {
  const t = (key: string) => `T(${key})`
  it("splits code and detail", () => {
    expect(parseMachineError("machines-unreachable: ssh: timed out")).toEqual({ code: "machines-unreachable", detail: "ssh: timed out" })
    expect(describeMachineError("machines-host-key", t)).toEqual({ code: "machines-host-key", message: "T(errors.machines-host-key)", detail: null })
  })
  it("falls back for unknown text and keeps it as detail", () => {
    expect(describeMachineError(new Error("boom"), t)).toEqual({ code: null, message: "T(errors.unknown)", detail: "boom" })
  })
  it("recognises machines-busy and herdr-operation-error", () => {
    expect(parseMachineError("herdr-operation-error: x").code).toBe("herdr-operation-error")
    expect(parseMachineError("machines-busy").code).toBe("machines-busy")
  })
})
