import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import {
  applyAgentSettled,
  applyAgentStart,
  applyBlocked,
  applySessionStart,
  desiredState,
  initialLifecycle,
  redactSessionId
} from "../lib/pi-lifecycle"

const testsDir = dirname(fileURLToPath(import.meta.url))
const adapter = readFileSync(join(testsDir, "../adapters/pi/yuzora-herdr-wsl.ts"), "utf8")
const blockedFixture = readFileSync(join(testsDir, "fixtures/pi-w09-block.ts"), "utf8")

describe("Pi lifecycle mapping", () => {
  it("replays start, working, blocked, idle without session reports", () => {
    let snap = initialLifecycle()
    expect(desiredState(snap).state).toBe("unknown")
    snap = applySessionStart(snap, "rpc", false)
    expect(snap.rootSession).toBe(false)
    snap = applySessionStart(snap, "tui", true)
    expect(desiredState(snap)).toEqual({ state: "idle" })
    snap = applyAgentStart(snap)
    expect(desiredState(snap)).toEqual({ state: "working" })
    snap = applyBlocked(snap, true, "permission")
    expect(desiredState(snap)).toEqual({ state: "blocked", message: "permission" })
    snap = applyBlocked(snap, true, "nested prompt")
    snap = applyBlocked(snap, false)
    expect(desiredState(snap).state).toBe("blocked")
    snap = applyBlocked(snap, false)
    expect(desiredState(snap)).toEqual({ state: "working" })
    snap = applyAgentSettled(snap, true)
    expect(desiredState(snap)).toEqual({ state: "idle" })
  })

  it("redacts native session ids for log-only evidence", () => {
    expect(redactSessionId("abc")).toContain("redacted")
    expect(redactSessionId("session-identifier-0001")).toMatch(/sess…01 \(len=/)
  })

  it("uses a real Pi UI prompt for W09 without faking Herdr reports", () => {
    expect(blockedFixture).toContain('registerCommand("yuzora-w09-block"')
    expect(blockedFixture).toContain("ctx.ui.confirm")
    expect(blockedFixture).not.toContain("herdr:blocked")
    expect(blockedFixture).not.toContain("report-agent")
  })

  it("keeps the installed adapter on CLI report-agent and log-only sessions", () => {
    expect(adapter).toContain('const SOURCE = "yuzora:wsl:pi"')
    expect(adapter).toContain("report-agent")
    expect(adapter).toContain("release-agent")
    expect(adapter).toContain("log-only, non-resumable")
    expect(adapter).not.toMatch(/report-agent-session/)
    expect(adapter).not.toContain("--agent-session-id")
    expect(adapter).toContain("Derived from Herdr v0.8.2")
    expect(adapter).toContain('ctx?.mode !== "tui"')
    expect(adapter).toContain("herdr:blocked")
    expect(adapter).toContain('pi.on("ui_prompt_start"')
    expect(adapter).toContain('pi.on("ui_prompt_end"')
    expect(adapter).toContain("uiPromptDepth")
    expect(adapter).toContain("agent_start")
    expect(adapter).toContain("agent_settled")
  })
})
