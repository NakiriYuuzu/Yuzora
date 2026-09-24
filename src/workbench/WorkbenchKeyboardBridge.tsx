import { useEffect } from "react"
import { dispatchAppShortcut } from "@/state/keyboardSettingsStore"
import { navigateWorkbenchTabs } from "@/lib/workbenchTabNavigation"
import { openNewTerminalTab } from "@/terminal/openNewTerminalTab"
import { useUiStore } from "@/state/uiStore"

export function WorkbenchKeyboardBridge() {
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      dispatchAppShortcut(event, "newTerminal", () => { void openNewTerminalTab() })
      dispatchAppShortcut(event, "toggleSidebarView", () => useUiStore.getState().requestSidebarViewToggle())
      dispatchAppShortcut(event, "nextTab", () => { void navigateWorkbenchTabs({ direction: 1 }) })
      dispatchAppShortcut(event, "previousTab", () => { void navigateWorkbenchTabs({ direction: -1 }) })
      for (const index of [1, 2, 3, 4, 5, 6, 7, 8, 9] as const) {
        dispatchAppShortcut(event, `tab${index}`, () => { void navigateWorkbenchTabs({ index: index - 1 }) })
      }
    }
    window.addEventListener("keydown", handler, true)
    return () => window.removeEventListener("keydown", handler, true)
  }, [])

  return null
}
