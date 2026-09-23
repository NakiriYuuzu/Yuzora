import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("@/lib/remoteFiles", () => ({
  registerRuntimeWorkspace: vi.fn(), listRemoteDir: vi.fn(),
  retainRemoteWorkspace: vi.fn(), releaseRemoteWorkspace: vi.fn(async () => {}),
}))
const { connection } = vi.hoisted(() => ({ connection: { owner: { hostId: "sqlite-lease", generation: 1 } } }))
vi.mock("@/state/hostStore", () => {
  const state = { configs: { "sqlite-lease": { hostId: "sqlite-lease", label: "Fixture" } }, hosts: { "sqlite-lease": { connection } } }
  return { useHostStore: Object.assign((selector: (value: any) => unknown) => selector(state), { getState: () => state }) }
})
import { listRemoteDir, registerRuntimeWorkspace, retainRemoteWorkspace, releaseRemoteWorkspace } from "@/lib/remoteFiles"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { SqliteLocationFields } from "./SqliteLocationFields"
const uri = "yuzora-fs://sqlite-lease@%2Frepo/repo"
const release = vi.fn(async () => {})
beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(registerRuntimeWorkspace).mockResolvedValue(uri)
  vi.mocked(listRemoteDir).mockResolvedValue([])
  vi.mocked(retainRemoteWorkspace).mockReturnValue(release)
})
afterEach(cleanup)
function openBrowser() {
  const view = render(<SqliteLocationFields path="" onPathChange={() => {}} workspace={{ hostId: "sqlite-lease", canonicalPath: "/repo" }} onWorkspaceChange={() => {}} disabled={false} onBrowseLocal={() => {}} />)
  fireEvent.click(screen.getByRole("button", { name: /browse/i }))
  return view
}
it("releases the temporary browser lease when the dialog unmounts", async () => {
  const view = openBrowser()
  await waitFor(() => expect(retainRemoteWorkspace).toHaveBeenCalledWith(uri))
  view.unmount()
  await waitFor(() => expect(releaseRemoteWorkspace).toHaveBeenCalledWith(uri))
  expect(release).toHaveBeenCalledTimes(1)
})
it("closes a capability whose registration completes after the dialog unmounts", async () => {
  let finish!: (uri: string) => void
  vi.mocked(registerRuntimeWorkspace).mockReturnValueOnce(new Promise(resolve => { finish = resolve }))
  const view = openBrowser()
  view.unmount()
  finish(uri)
  await waitFor(() => expect(releaseRemoteWorkspace).toHaveBeenCalledWith(uri))
  expect(release).toHaveBeenCalledTimes(1)
  expect(listRemoteDir).not.toHaveBeenCalled()
})
it("browses a Windows host workspace and walks back to its parent folder", async () => {
  const root = String.raw`C:\Work`
  const workspaceUri = remoteFilePath("sqlite-lease", root)
  const child = remoteFilePath("sqlite-lease", String.raw`C:\Work\data`, root)
  vi.mocked(registerRuntimeWorkspace).mockResolvedValue(workspaceUri)
  vi.mocked(listRemoteDir).mockResolvedValue([{ name: "data", path: child, isDir: true }])
  render(<SqliteLocationFields path="" onPathChange={() => {}} workspace={{ hostId: "sqlite-lease", canonicalPath: root }} onWorkspaceChange={() => {}} disabled={false} onBrowseLocal={() => {}} />)
  fireEvent.click(screen.getByRole("button", { name: /browse/i }))
  fireEvent.click(await screen.findByRole("button", { name: "data" }))
  await waitFor(() => expect(listRemoteDir).toHaveBeenLastCalledWith(child))
  fireEvent.click(await screen.findByRole("button", { name: ".." }))
  await waitFor(() => expect(listRemoteDir).toHaveBeenLastCalledWith(workspaceUri))
})
