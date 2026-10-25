import { expect, it } from "vitest"
import { companionAgo } from "./companionAgo"

const now = Date.parse("2026-10-20T12:00:00Z")
const ago = (seconds: number, locale = "en") => companionAgo(new Date(now - seconds * 1000).toISOString(), now, locale)

it("counts to the minute, hour and day", () => {
  expect(ago(5)).toBe("now")
  expect(ago(59)).toBe("now")
  expect(ago(60)).toBe("1 minute ago")
  expect(ago(5 * 60 + 59)).toBe("5 minutes ago")
  expect(ago(2 * 3600)).toBe("2 hours ago")
  expect(ago(86_400)).toBe("yesterday")
  expect(ago(3 * 86_400)).toBe("3 days ago")
})

it("follows the locale", () => {
  expect(ago(5 * 60, "zh-TW")).toBe("5 分鐘前")
})

it("reads the gateway's nanosecond timestamps, and a time ahead of the clock as now", () => {
  expect(companionAgo("2026-10-20T11:54:59.176376458Z", now, "en")).toBe("5 minutes ago")
  expect(companionAgo("2026-10-20T12:00:30Z", now, "en")).toBe("now")
})

it("says nothing about something that is not a date", () => {
  expect(companionAgo("later", now, "en")).toBeNull()
})
