import { expect, it } from "vitest"
import { agentLogoMarkup, resolveAgentKind } from "./agentLogos"

it("maps HERDR agent labels to bundled logos", () => {
  expect(resolveAgentKind("Claude Code")).toBe("claude")
  expect(resolveAgentKind(null, "codex")).toBe("codex")
  expect(resolveAgentKind("Gemini CLI")).toBe("gemini")
  expect(resolveAgentKind("GitHub Copilot")).toBe("copilot")
  expect(resolveAgentKind("Pi")).toBe("pi")
  expect(resolveAgentKind("Kilo Code")).toBe("kilo")
  expect(resolveAgentKind("pixel shell")).toBeNull()
  expect(resolveAgentKind("bash")).toBeNull()
  expect(agentLogoMarkup("codex")).toContain("<svg")
  expect(agentLogoMarkup("droid")).toBeNull()
})
