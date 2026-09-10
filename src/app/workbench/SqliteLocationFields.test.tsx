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
