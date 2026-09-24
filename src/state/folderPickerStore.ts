import { create } from "zustand"
import { open as openNativeFolderDialog } from "@tauri-apps/plugin-dialog"
import { isWindowsPlatform } from "@/lib/platform"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"

interface FolderPickerState {
    open: boolean
    initialLocation?: "local" | "remote" | "wsl"
    legacyWindowsPath?: string
    runtimeHostId?: string
  finish: ((path: string | null) => void) | null
}
export const useFolderPickerStore = create<FolderPickerState>(() => ({ open: false, finish: null }))

type FolderPickerOptions = { initialLocation?: "local" | "remote" | "wsl"; legacyWindowsPath?: string; runtimeHostId?: string }

/** A local-only choice has nothing to configure, so the OS folder dialog opens without an app modal. */
function opensNativeDirectly(options?: FolderPickerOptions) {
  // Windows keeps the modal: it is where users choose Windows-native versus WSL.
  if (options?.legacyWindowsPath || isWindowsPlatform()) return false
  if (options?.runtimeHostId) return options.runtimeHostId === LOCAL_HOST_ID
  return (options?.initialLocation ?? "local") === "local"
}

let nativeDialogOpen = false

export function chooseWorkspaceFolder(options?: FolderPickerOptions): Promise<string | null> {
  const current = useFolderPickerStore.getState()
  if (current.open || nativeDialogOpen) return Promise.resolve(null)
  if (opensNativeDirectly(options)) {
    nativeDialogOpen = true
    return openNativeFolderDialog({ directory: true, multiple: false })
      .then((selected) => typeof selected === "string" ? selected : null)
      .finally(() => { nativeDialogOpen = false })
  }
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
