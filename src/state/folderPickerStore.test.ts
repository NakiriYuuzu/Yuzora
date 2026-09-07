import { afterEach, expect, it } from "vitest"
import { chooseWorkspaceFolder, useFolderPickerStore } from "./folderPickerStore"

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
