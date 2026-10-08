import { describe, expect, it } from "vitest"
import { buildMachineTarget } from "./machinesTarget"

describe("buildMachineTarget", () => {
  it.each([
    [{ user: "me", host: "box.example", port: 22 }, "me@box.example"],
    [{ user: "me", host: "box.example", port: 2222 }, "ssh://me@box.example:2222"],
    [{ user: "", host: "box.example", port: 22 }, "box.example"],
    [{ user: "", host: "box.example", port: 2200 }, "ssh://box.example:2200"],
    [{ user: "me", host: "::1", port: 22 }, "ssh://me@[::1]"],
    [{ user: "me", host: "fe80::1", port: 2222 }, "ssh://me@[fe80::1]:2222"],
    [{ user: " me ", host: " box.example ", port: 22 }, "me@box.example"]
  ])("%j -> %s", (host, expected) => {
    expect(buildMachineTarget(host)).toBe(expected)
  })
})
