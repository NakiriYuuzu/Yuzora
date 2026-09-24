import { afterEach, beforeEach, expect, it, vi } from "vitest"

const dialog = vi.hoisted(() => ({ open: vi.fn() }))
const platform = vi.hoisted(() => ({ windows: false }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }))
vi.mock("@/lib/platform", () => ({ isWindowsPlatform: () => platform.windows }))

import { chooseWorkspaceFolder, useFolderPickerStore } from "./folderPickerStore"

beforeEach(() => { dialog.open.mockReset(); platform.windows = false })
afterEach(() => useFolderPickerStore.getState().finish?.(null))

it("late selection from a cancelled picker cannot dismiss its replacement", async () => {
  const first = chooseWorkspaceFolder({ runtimeHostId: "host-a" })
  const finishFirst = useFolderPickerStore.getState().finish!
  finishFirst(null)
  expect(await first).toBeNull()
  const second = chooseWorkspaceFolder({ runtimeHostId: "host-b" })
  finishFirst("/late")
  expect(useFolderPickerStore.getState()).toMatchObject({ open: true, runtimeHostId: "host-b" })
  useFolderPickerStore.getState().finish!("/current")
  expect(await second).toBe("/current")
})

it("opens the system folder dialog directly for a local Space without an app modal", async () => {
  dialog.open.mockResolvedValue("/Users/me/project")
  await expect(chooseWorkspaceFolder({ runtimeHostId: "local" })).resolves.toBe("/Users/me/project")
  expect(dialog.open).toHaveBeenCalledWith({ directory: true, multiple: false })
  expect(useFolderPickerStore.getState().open).toBe(false)
})

it("returns null when the direct system dialog is cancelled", async () => {
  dialog.open.mockResolvedValue(null)
  await expect(chooseWorkspaceFolder()).resolves.toBeNull()
  expect(useFolderPickerStore.getState().open).toBe(false)
})

it("keeps the app modal where a location choice is required", async () => {
  platform.windows = true
  void chooseWorkspaceFolder()
  expect(useFolderPickerStore.getState()).toMatchObject({ open: true, initialLocation: "local" })
  useFolderPickerStore.getState().finish!(null)
  void chooseWorkspaceFolder({ initialLocation: "remote" })
  expect(useFolderPickerStore.getState()).toMatchObject({ open: true, initialLocation: "remote" })
  useFolderPickerStore.getState().finish!(null)
  void chooseWorkspaceFolder({ runtimeHostId: "local" })
  expect(useFolderPickerStore.getState()).toMatchObject({ open: true, runtimeHostId: "local" })
  expect(dialog.open).not.toHaveBeenCalled()
})
