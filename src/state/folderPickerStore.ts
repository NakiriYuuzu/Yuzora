import { create } from "zustand"

interface FolderPickerState {
    open: boolean
    initialLocation?: "local" | "remote" | "wsl"
    legacyWindowsPath?: string
    runtimeHostId?: string
  finish: ((path: string | null) => void) | null
}
export const useFolderPickerStore = create<FolderPickerState>(() => ({ open: false, finish: null }))

export function chooseWorkspaceFolder(options?: { initialLocation?: "local" | "remote" | "wsl"; legacyWindowsPath?: string; runtimeHostId?: string }): Promise<string | null> {
  const current = useFolderPickerStore.getState()
  if (current.open) return Promise.resolve(null)
  return new Promise((resolve) => {
    const finish = (path: string | null) => {
      if (useFolderPickerStore.getState().finish !== finish) return
      useFolderPickerStore.setState({ open: false, finish: null })
      resolve(path)
    }
    useFolderPickerStore.setState({
      open: true,
      initialLocation: options?.initialLocation ?? "local",
      legacyWindowsPath: options?.legacyWindowsPath,
      runtimeHostId: options?.runtimeHostId,
      finish
    })
  })
}
