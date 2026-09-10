import { afterEach, expect, it, vi } from "vitest"
import { useHostStore } from "@/state/hostStore"
import { remoteFilePath } from "./runtimeIdentity"
import { revealPathInSystem, systemRevealPath } from "./revealPath"

const opener = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: opener }))
afterEach(() => { vi.restoreAllMocks(); useHostStore.setState({ hosts: {} }); opener.mockClear() })

it("reveals WSL files in their connected distribution with Unicode and spaces intact", async () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
  useHostStore.setState({ hosts: { "wsl:identity": {
    connection: { owner: { hostId: "wsl:identity", generation: 1 }, hello: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/home/me", methods: [] } },
    target: { kind: "wsl", distro: "Ubuntu-24.04" }, connecting: false, error: null, attempt: 0, retryAt: 0
  } } })
  await revealPathInSystem(remoteFilePath("wsl:identity", "/home/me/中文 project/a.ts", "/home/me/中文 project"))
  expect(opener).toHaveBeenCalledWith(String.raw`\\wsl.localhost\Ubuntu-24.04\home\me\中文 project\a.ts`)
  expect(systemRevealPath(remoteFilePath("another-host", "/same/path"))).toBeNull()
})

it("keeps local paths and refuses disconnected remote identities", async () => {
  await revealPathInSystem("C:\\Project\\a.ts")
  expect(opener).toHaveBeenCalledWith("C:\\Project\\a.ts")
  await expect(revealPathInSystem(remoteFilePath("ssh-host", "/project/a.ts"))).rejects.toThrow("unavailable")
  expect(opener).toHaveBeenCalledTimes(1)
})
