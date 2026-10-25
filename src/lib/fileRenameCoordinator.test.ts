import { afterEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { openFile, saveFile } from "./ipc"
import { remoteFilePath } from "./runtimeIdentity"
import { reserveFileRename } from "./fileRenameCoordinator"

const remote = vi.hoisted(() => ({ readRemoteFile: vi.fn(), saveRemoteFile: vi.fn() }))
vi.mock("./remoteFiles", () => remote)
const releases: Array<() => void> = []
function reserve(from: string, to: string) {
    const release = reserveFileRename(from, to)
    releases.push(release)
    return release
}
function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>(done => { resolve = done })
    return { promise, resolve }
}
afterEach(() => {
    for (const release of releases.splice(0)) release()
    clearMocks(); vi.resetAllMocks()
})

it("serializes remote reads across overlapping capability roots on the same host", async () => {
    const release = reserve(remoteFilePath("host", "/repo/sub/a", "/repo"), remoteFilePath("host", "/repo/sub/b", "/repo"))
    const alias = remoteFilePath("host", "/repo/sub/b/file.txt", "/repo/sub")
    const result = { kind: "full", content: "REMOTE", size: 6, lineEnding: "lf" }
    remote.readRemoteFile.mockResolvedValue(result)
    const pending = openFile(alias)
    await Promise.resolve()
    expect(remote.readRemoteFile).not.toHaveBeenCalled()
    await expect(saveFile(alias, "NO_WRITE")).rejects.toThrow()
    expect(remote.saveRemoteFile).not.toHaveBeenCalled()
    release()
    expect(await pending).toBe(result)
    expect(remote.readRemoteFile).toHaveBeenCalledWith(alias)
})

it("keeps other hosts, sibling prefixes and case-sensitive remote POSIX files usable", async () => {
    reserve(remoteFilePath("host", "/repo/A", "/repo"), remoteFilePath("host", "/repo/B", "/repo"))
    remote.saveRemoteFile.mockResolvedValue(1)
    for (const path of [remoteFilePath("other", "/repo/A/file", "/repo"), remoteFilePath("host", "/repo/AA/file", "/repo"), remoteFilePath("host", "/repo/a/file", "/repo")]) {
        await expect(saveFile(path, "x")).resolves.toBe(1)
    }
})

it("recognizes Windows remote path aliases across overlapping capability roots", async () => {
    reserve(remoteFilePath("host", "C:/Repo/Old", "C:/Repo"), remoteFilePath("host", "C:/Repo/New", "C:/Repo"))
    await expect(saveFile(remoteFilePath("host", "c:/repo/new/file.txt", "C:/Repo/New"), "x")).rejects.toThrow()
    expect(remote.saveRemoteFile).not.toHaveBeenCalled()
})

it("holds admission through an already-started remote save and releases rejection paths", async () => {
    const path = remoteFilePath("host", "/repo/a", "/repo")
    const gate = deferred<number>()
    remote.saveRemoteFile.mockReturnValue(gate.promise)
    const writing = saveFile(path, "x")
    expect(() => reserve(path, remoteFilePath("host", "/repo/b", "/repo"))).toThrow()
    gate.resolve(1); await writing
    reserve(path, remoteFilePath("host", "/repo/b", "/repo"))()
    remote.saveRemoteFile.mockRejectedValue(new Error("remote failed"))
    await expect(saveFile(path, "x")).rejects.toThrow("remote failed")
    reserve(path, remoteFilePath("host", "/repo/b", "/repo"))()
})

it("releases local read and write admission on errors", async () => {
    mockIPC(() => { throw new Error("fixture failure") })
    await expect(openFile("/fixture/a")).rejects.toThrow("fixture failure")
    await expect(saveFile("/fixture/a", "x")).rejects.toThrow("fixture failure")
    reserve("/fixture/a", "/fixture/b")()
})

it("does not release a newer reservation when an older release runs again", async () => {
    const release = reserve("/fixture/a", "/fixture/b")
    release()
    const newer = reserve("/fixture/a", "/fixture/b")
    release()
    await expect(saveFile("/fixture/a", "x")).rejects.toThrow()
    newer()
})
