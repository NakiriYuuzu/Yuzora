import { expect, it } from "vitest"
import i18n from "@/lib/i18n"
import { describeCompanionError } from "./companionErrors"

const t = i18n.getFixedT("en", "companion")

it("localizes known codes and keeps their detail", () => {
  expect(describeCompanionError("companion-start-timeout", t)).toBe("The Companion did not start in time.")
  expect(describeCompanionError("tailscale-failed: serve config denied", t)).toBe("The Tailscale command failed. (serve config denied)")
  expect(describeCompanionError("companion-tailscale-refused: httpsNotReady", t)).toMatch(/^HTTPS certificates are not enabled/)
})

it("shows unknown codes and free text as sent", () => {
  expect(describeCompanionError("companion-something-new", t)).toBe("companion-something-new")
  expect(describeCompanionError("companion-tailscale-refused: later", t)).toBe("companion-tailscale-refused: later")
  expect(describeCompanionError("Permission denied (os error 13)", t)).toBe("Permission denied (os error 13)")
})
