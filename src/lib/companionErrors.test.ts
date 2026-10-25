import { expect, it } from "vitest"
import i18n from "@/lib/i18n"
import { describeCompanionError } from "./companionErrors"

const t = i18n.getFixedT("en", "companion")

it("localizes known codes and keeps their detail", () => {
  expect(describeCompanionError("companion-start-timeout", t)).toBe("The Companion did not start in time.")
  expect(describeCompanionError("companion-net-start-failed: No such file or directory", t)).toBe("The Tailscale connection could not start. (No such file or directory)")
  expect(describeCompanionError("companion-net-https-disabled", t)).toMatch(/^HTTPS certificates are not enabled/)
})

it("shows unknown codes and free text as sent", () => {
  expect(describeCompanionError("companion-something-new", t)).toBe("companion-something-new")
  expect(describeCompanionError("companion-net-later: detail", t)).toBe("companion-net-later: detail")
  expect(describeCompanionError("Permission denied (os error 13)", t)).toBe("Permission denied (os error 13)")
})

it("names the configured port of a taken port, in both languages", () => {
  expect(describeCompanionError("port-in-use", t, 47999)).toBe("Another program is using port 47999, which the Companion needs. Quit that program and try again.")
  const zh = i18n.getFixedT("zh-TW", "companion")
  expect(describeCompanionError("port-in-use", zh, 47999)).toBe("另一個程式正在使用 Companion 需要的連接埠 47999。請關閉該程式後再試一次。")
})

it("keeps the generic text of a taken port while no port is known", () => {
  const generic = "Another program is using the Companion's port. Quit that program and try again."
  expect(describeCompanionError("port-in-use", t)).toBe(generic)
  expect(describeCompanionError("port-in-use", t, null)).toBe(generic)
  expect(describeCompanionError("port-in-use", t, 0)).toBe(generic)
  expect(describeCompanionError("listen-failed", t)).toBe("The Companion could not start listening on its port. Try again; if it keeps failing, restart Yuzora.")
})

/** Every code the desktop or the gateway can raise that a user may see, as the desktop reviews listed the gaps. */
const RAISED = [
  "companion-cancelled",
  "companion-device-invalid",
  "companion-invalid-reply",
  "companion-net-approval-required",
  "companion-path-unsupported",
  "companion-qr-failed",
  "companion-space-roots-invalid",
  "companion-push-encrypt-failed",
  "companion-push-subscription-invalid",
  "companion-push-vault-corrupt",
  "companion-push-vault-unavailable",
  "companion-push-vault-write-failed",
  "companion-error",
  "invalid-command",
  "invalid-trust-store",
  "invalid-space-roots",
  "invalid-host",
  "invalid-login",
  "invalid-proxy-secret",
  "invalid-herdr-binary",
  "invalid-frame",
  "invalid-policy",
  "init-timeout",
  "invalid-web-root",
  "invalid-devices",
  "invalid-push",
  "lan-not-supported",
  "not-initialized",
  "unknown-type",
  "already-initialized",
  "admin-queue-unavailable",
  "admin-command-rejected",
  "port-in-use",
  "listen-failed",
]

it.each(["en", "zh-TW"])("localizes every code the Companion can raise in %s", locale => {
  const translate = i18n.getFixedT(locale, "companion")
  for (const code of RAISED) {
    const message = describeCompanionError(code, translate)
    expect(message, code).not.toBe(code)
    expect(message, code).not.toContain("{{")
    expect(describeCompanionError(code, translate, 47999), code).not.toContain("{{")
    if (locale === "zh-TW") expect(message, code).toMatch(/[一-鿿]/)
  }
})
