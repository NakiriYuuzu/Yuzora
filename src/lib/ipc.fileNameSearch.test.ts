import { afterEach, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { searchWorkspaceFileNames } from "./ipc"

afterEach(() => clearMocks())

it("searchWorkspaceFileNames invokes the native filename command and preserves its result shape", async () => {
  const result = { files: [{ name: "Target.ts", path: "C:\\Work\\Target.ts", isDir: false, kind: "file" }], incomplete: true }
  mockIPC((command, args) => {
    expect(command).toBe("search_workspace_file_names")
    expect(args).toEqual({ root: "C:\\Work", query: "target" })
    return result
  })
  expect(await searchWorkspaceFileNames("C:\\Work", "target")).toEqual(result)
})
