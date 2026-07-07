import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Command as CommandPrimitive } from "cmdk"
import {
  ListTreeIcon,
  MonitorPlayIcon,
  SearchIcon,
  SettingsIcon,
  SquareTerminalIcon,
  WaypointsIcon,
  type LucideIcon,
} from "lucide-react"

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
import { useOverlayPresence } from "@/state/overlayStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { SymbolPicker } from "@/workbench/SymbolPicker"
import { useWorkspaceSearch } from "@/workbench/search/useWorkspaceSearch"
import { WorkspaceSearchGroup } from "@/workbench/search/WorkspaceSearchGroup"

interface CommandPaletteProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSelectMode: (mode: Mode) => void
  onOpenSettings: () => void
}

const ITEM_CLASS =
  "h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"

/**
 * Command palette — design reference 5.9. Global ⌘K / Ctrl+K toggle. VSCode-style
 * modes: a leading ">" restricts the list to commands (filtered by the text after
 * it); any other non-empty query keeps the matching commands and adds an async
 * "工作區搜尋" (workspace full-text) group. cmdk's built-in filter would drop the
 * async result rows (their values don't match the typed query), so filtering is
 * disabled (`shouldFilter={false}`) and the command list is filtered by hand;
 * the workspace rows are cmdk items so keyboard nav still reaches them.
 */
export function CommandPalette({ open, onOpenChange, onSelectMode, onOpenSettings }: CommandPaletteProps) {
  const { t } = useTranslation("workbench")
  // Picker open state is kept separate from mode (rather than a nullable mode) so
  // SymbolPicker can stay permanently mounted — its reset-on-close effect only
  // runs when it isn't unmounted on every close.
  // Register with the preview child-webview z-order gate: while the palette is
  // open the native webview must hide so it can't paint over this dialog.
  useOverlayPresence(open)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [pickerMode, setPickerMode] = useState<"document" | "workspace">("document")
  const [search, setSearch] = useState("")
  const [caseSensitive, setCaseSensitive] = useState(false)
  const toggleTerminal = useUiStore((s) => s.toggleTerminal)
  const togglePreviewTab = useWorkspaceStore((s) => s.togglePreviewTab)
  const requestReveal = useWorkspaceStore((s) => s.requestReveal)

  const isCommandMode = search.startsWith(">")
  const commandFilter = (isCommandMode ? search.slice(1) : search).trim().toLowerCase()
  const matchesCommand = (label: string) =>
    commandFilter === "" || label.toLowerCase().includes(commandFilter)
  // ">" is command-only; any other query of at least 2 chars also runs a
  // workspace search (the 2-char floor keeps a single keystroke from scanning
  // the whole tree — mirrored in useWorkspaceSearch).
  const showWorkspace = !isCommandMode && search.trim().length >= 2

  // Discard/cancel in-flight responses whenever the palette is closed or the
  // query switches to command-only mode by feeding the hook an empty query.
  const { events, loading } = useWorkspaceSearch(open && showWorkspace ? search : "", caseSensitive)

  type Cmd = { value: string; label: string; icon: LucideIcon; onSelect: () => void; className: string }
  const commands: Cmd[] = [
    ...MODES.map((m) => ({
      value: m.label,
      label: m.label,
      icon: m.icon,
      onSelect: () => {
        onSelectMode(m.id)
        onOpenChange(false)
      },
      className: ITEM_CLASS,
    })),
    {
      value: t("commandPalette.goToSymbol"),
      label: t("commandPalette.goToSymbol"),
      icon: ListTreeIcon,
      onSelect: () => {
        onOpenChange(false)
        setPickerMode("document")
        setPickerOpen(true)
      },
      className: ITEM_CLASS,
    },
    {
      value: t("commandPalette.workspaceSymbols"),
      label: t("commandPalette.workspaceSymbols"),
      icon: WaypointsIcon,
      onSelect: () => {
        onOpenChange(false)
        setPickerMode("workspace")
        setPickerOpen(true)
      },
      className: ITEM_CLASS,
    },
    {
      value: t("commandPalette.toggleTerminal"),
      label: t("commandPalette.toggleTerminal"),
      icon: SquareTerminalIcon,
      onSelect: () => {
        toggleTerminal()
        onOpenChange(false)
      },
      className: ITEM_CLASS,
    },
    {
      value: t("commandPalette.togglePreview"),
      label: t("commandPalette.togglePreview"),
      icon: MonitorPlayIcon,
      onSelect: () => {
        togglePreviewTab()
        onOpenChange(false)
      },
      className: ITEM_CLASS,
    },
    {
      value: t("commandPalette.settings"),
      label: t("commandPalette.settings"),
      icon: SettingsIcon,
      onSelect: () => {
        onOpenSettings()
        onOpenChange(false)
      },
      className: "h-[42px] gap-3",
    },
  ]
  const visibleCommands = commands.filter((c) => matchesCommand(c.label))

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

  // Start every open from a clean query; closing also lets the search hook cancel.
  useEffect(() => {
    if (!open) setSearch("")
  }, [open])

  return (
    <>
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader className="sr-only">
        <DialogTitle>{t("commandPalette.title")}</DialogTitle>
        <DialogDescription>{t("commandPalette.searchPlaceholder")}</DialogDescription>
      </DialogHeader>
      <DialogContent
        showCloseButton={false}
        className="yz-diffin top-[15vh] w-[620px] max-w-[88vw] translate-y-0 gap-0 overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--frost-light) p-0 shadow-(--shadow-xl) ring-0 backdrop-blur-[20px] backdrop-saturate-[1.5] sm:max-w-[88vw]"
      >
        <Command shouldFilter={false} className="rounded-none! bg-transparent p-0">
          <div className="flex items-center gap-3 border-b border-(--line-1) px-[21px] py-[17px]">
            <SearchIcon className="size-5 shrink-0 text-(--ink-3)" aria-hidden="true" />
            <CommandPrimitive.Input
              autoFocus
              value={search}
              onValueChange={setSearch}
              placeholder={t("commandPalette.searchPlaceholder")}
              className="flex-1 bg-transparent text-[19px] font-medium text-(--ink-1) outline-none placeholder:text-(--ink-3)"
            />
            <kbd className="shrink-0 rounded-[6px] bg-(--yz-active) px-[8px] py-[3px] font-mono text-[11px] text-(--ink-3)">
              esc
            </kbd>
          </div>

          <CommandList className="yzs max-h-[398px] p-[8px]">
            {!showWorkspace && (
              <CommandEmpty className="py-6 text-center text-[13px] text-(--ink-3)">
                {t("commandPalette.noCommandsMatch")}
              </CommandEmpty>
            )}
            {visibleCommands.length > 0 && (
              <CommandGroup
                heading={t("commandPalette.goToHeading")}
                className="**:[[cmdk-group-heading]]:text-[10px] **:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-wider **:[[cmdk-group-heading]]:text-(--ink-3) **:[[cmdk-group-heading]]:uppercase"
              >
                {visibleCommands.map((c) => {
                  const Icon = c.icon
                  return (
                    <CommandItem
                      key={c.value}
                      value={c.value}
                      onSelect={c.onSelect}
                      className={c.className}
                    >
                      <span className="flex size-[28px] shrink-0 items-center justify-center rounded-[9px] bg-(--yz-hover)">
                        <Icon className="size-[16px]" aria-hidden="true" />
                      </span>
                      <span className="text-[14px] font-medium">{c.label}</span>
                    </CommandItem>
                  )
                })}
              </CommandGroup>
            )}
            {showWorkspace && (
              <WorkspaceSearchGroup
                events={events}
                query={search.trim()}
                loading={loading}
                caseSensitive={caseSensitive}
                onToggleCaseSensitive={() => setCaseSensitive((v) => !v)}
                onReveal={(path, line) => {
                  requestReveal(path, line)
                  onOpenChange(false)
                }}
              />
            )}
          </CommandList>

          <div className="flex items-center gap-[16px] border-t border-(--line-1) bg-(--yz-hover) px-[18px] py-[10px] text-[11px] text-(--ink-3)">
            <span className="flex items-center gap-[5px]">
              <kbd className="rounded-[4px] bg-(--yz-active) px-[6px] py-px font-mono">↑↓</kbd>
              {t("commandPalette.navigateHint")}
            </span>
            <span className="flex items-center gap-[5px]">
              <kbd className="rounded-[4px] bg-(--yz-active) px-[6px] py-px font-mono">⏎</kbd>
              {t("commandPalette.runHint")}
            </span>
            <div className="flex-1" />
            <span className="font-serif text-[12px] italic text-(--yz-accent-ink)">
              {t("commandPalette.tagline")}
            </span>
          </div>
        </Command>
      </DialogContent>
    </Dialog>

    <SymbolPicker open={pickerOpen} onOpenChange={setPickerOpen} mode={pickerMode} />
    </>
  )
}
