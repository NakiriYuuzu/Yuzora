import { describe, expect, it } from "vitest"

import { fullDateTime, relativeTime } from "./relativeTime"

// Fixed reference clock: 2026-07-03 12:00:00 local.
const NOW = new Date(2026, 6, 3, 12, 0, 0)
const nowSec = Math.floor(NOW.getTime() / 1000)

function ago(seconds: number): number {
    return nowSec - seconds
}

describe("relativeTime", () => {
    it("shows 'now' for sub-minute and future timestamps", () => {
        expect(relativeTime(ago(0), NOW)).toBe("now")
        expect(relativeTime(ago(30), NOW)).toBe("now")
        expect(relativeTime(ago(-10), NOW)).toBe("now")
    })

    it("shows minutes under an hour", () => {
        expect(relativeTime(ago(5 * 60), NOW)).toBe("5m")
        expect(relativeTime(ago(59 * 60), NOW)).toBe("59m")
    })

    it("shows hours under a day", () => {
        expect(relativeTime(ago(3 * 3600), NOW)).toBe("3h")
        expect(relativeTime(ago(23 * 3600), NOW)).toBe("23h")
    })

    it("shows days under a week", () => {
        expect(relativeTime(ago(2 * 86400), NOW)).toBe("2d")
        expect(relativeTime(ago(6 * 86400), NOW)).toBe("6d")
    })

    it("shows weeks under 30 days", () => {
        expect(relativeTime(ago(7 * 86400), NOW)).toBe("1w")
        expect(relativeTime(ago(21 * 86400), NOW)).toBe("3w")
        expect(relativeTime(ago(29 * 86400), NOW)).toBe("4w")
    })

    it("shows a short date at 30 days and beyond", () => {
        // 40 days before 2026-07-03 → 2026-05-24.
        expect(relativeTime(ago(40 * 86400), NOW)).toBe("May 24")
    })
})

describe("fullDateTime", () => {
    it("formats a zero-padded local date-time string", () => {
        const ts = Math.floor(new Date(2026, 6, 3, 14, 22, 0).getTime() / 1000)
        expect(fullDateTime(ts)).toBe("2026-07-03 14:22")
    })

    it("zero-pads single-digit month / day / hour / minute", () => {
        const ts = Math.floor(new Date(2026, 0, 5, 9, 7, 0).getTime() / 1000)
        expect(fullDateTime(ts)).toBe("2026-01-05 09:07")
    })
})
