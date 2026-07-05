import { useEffect, useState } from "react"
import { Command as CommandPrimitive } from "cmdk"
import { ListTreeIcon, SearchIcon, SettingsIcon, WaypointsIcon } from "lucide-react"

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { MODES, type Mode } from "@/app/modes"
import { SymbolPicker } from "@/workbench/SymbolPicker"

interface CommandPaletteProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSelectMode: (mode: Mode) => void
  onOpenSettings: () => void
}

/**
 * Command palette — design reference 5.9. Global ⌘K / Ctrl+K toggle, "Go to"
 * group listing the 5 modes plus Settings.
 */
export function CommandPalette({ open, onOpenChange, onSelectMode, onOpenSettings }: CommandPaletteProps) {
  // Picker open state is kept separate from mode (rather than a nullable mode) so
  // SymbolPicker can stay permanently mounted — its reset-on-close effect only
  // runs when it isn't unmounted on every close.
  const [pickerOpen, setPickerOpen] = useState(false)
  const [pickerMode, setPickerMode] = useState<"document" | "workspace">("document")

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        // The symbol picker sits above the palette; ⌘K there closes it rather than
        // stacking a second dialog on top.
        if (pickerOpen) {
          setPickerOpen(false)
          return
        }
        onOpenChange(!open)
      }
    }
    window.addEventListener("keydown", handler)
    return () => window.removeEventListener("keydown", handler)
  }, [open, onOpenChange, pickerOpen])

  return (
    <>
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader className="sr-only">
        <DialogTitle>Command Palette</DialogTitle>
        <DialogDescription>Search files, run a command…</DialogDescription>
      </DialogHeader>
      <DialogContent
        showCloseButton={false}
        className="yz-diffin top-[15vh] w-[620px] max-w-[88vw] translate-y-0 gap-0 overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--frost-light) p-0 shadow-(--shadow-xl) ring-0 backdrop-blur-[20px] backdrop-saturate-[1.5] sm:max-w-[88vw]"
      >
        <Command className="rounded-none! bg-transparent p-0">
          <div className="flex items-center gap-3 border-b border-(--line-1) px-[21px] py-[17px]">
            <SearchIcon className="size-5 shrink-0 text-(--ink-3)" aria-hidden="true" />
            <CommandPrimitive.Input
              autoFocus
              placeholder="Search files, run a command…"
              className="flex-1 bg-transparent text-[19px] font-medium text-(--ink-1) outline-none placeholder:text-(--ink-3)"
            />
            <kbd className="shrink-0 rounded-[6px] bg-(--yz-active) px-[8px] py-[3px] font-mono text-[11px] text-(--ink-3)">
              esc
            </kbd>
          </div>

          <CommandList className="yzs max-h-[398px] p-[8px]">
            <CommandEmpty className="py-6 text-center text-[13px] text-(--ink-3)">
              No commands match
            </CommandEmpty>
            <CommandGroup
              heading="Go to"
              className="**:[[cmdk-group-heading]]:text-[10px] **:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-wider **:[[cmdk-group-heading]]:text-(--ink-3) **:[[cmdk-group-heading]]:uppercase"
            >
              {MODES.map((m) => {
                const Icon = m.icon
                return (
                  <CommandItem
                    key={m.id}
                    value={m.label}
                    onSelect={() => {
                      onSelectMode(m.id)
                      onOpenChange(false)
                    }}
                    className="h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"
                  >
                    <span className="flex size-[28px] shrink-0 items-center justify-center rounded-[9px] bg-(--yz-hover)">
                      <Icon className="size-[16px]" aria-hidden="true" />
                    </span>
                    <span className="text-[14px] font-medium">{m.label}</span>
                  </CommandItem>
                )
              })}
              <CommandItem
                value="Go to symbol"
                onSelect={() => {
                  onOpenChange(false)
                  setPickerMode("document")
                  setPickerOpen(true)
                }}
                className="h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"
              >
                <span className="flex size-[28px] shrink-0 items-center justify-center rounded-[9px] bg-(--yz-hover)">
                  <ListTreeIcon className="size-[16px]" aria-hidden="true" />
                </span>
                <span className="text-[14px] font-medium">Go to symbol</span>
              </CommandItem>
              <CommandItem
                value="Workspace symbols"
                onSelect={() => {
                  onOpenChange(false)
                  setPickerMode("workspace")
                  setPickerOpen(true)
                }}
                className="h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"
              >
                <span className="flex size-[28px] shrink-0 items-center justify-center rounded-[9px] bg-(--yz-hover)">
                  <WaypointsIcon className="size-[16px]" aria-hidden="true" />
                </span>
                <span className="text-[14px] font-medium">Workspace symbols</span>
              </CommandItem>
              <CommandItem
                value="Settings"
                onSelect={() => {
                  onOpenSettings()
                  onOpenChange(false)
                }}
                className="h-[42px] gap-3"
              >
                <span className="flex size-[28px] shrink-0 items-center justify-center rounded-[9px] bg-(--yz-hover)">
                  <SettingsIcon className="size-[16px]" aria-hidden="true" />
                </span>
                <span className="text-[14px] font-medium">Settings</span>
              </CommandItem>
            </CommandGroup>
          </CommandList>

          <div className="flex items-center gap-[16px] border-t border-(--line-1) bg-(--yz-hover) px-[18px] py-[10px] text-[11px] text-(--ink-3)">
            <span className="flex items-center gap-[5px]">
              <kbd className="rounded-[4px] bg-(--yz-active) px-[6px] py-px font-mono">↑↓</kbd>
              navigate
            </span>
            <span className="flex items-center gap-[5px]">
              <kbd className="rounded-[4px] bg-(--yz-active) px-[6px] py-px font-mono">⏎</kbd>
              run
            </span>
            <div className="flex-1" />
            <span className="font-serif text-[12px] italic text-(--yz-accent-ink)">
              Yuuzu command center
            </span>
          </div>
        </Command>
      </DialogContent>
    </Dialog>

    <SymbolPicker open={pickerOpen} onOpenChange={setPickerOpen} mode={pickerMode} />
    </>
  )
}
