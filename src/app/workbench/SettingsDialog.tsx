import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { getVersion } from "@tauri-apps/api/app"
import changelogMarkdown from "../../../CHANGELOG.md?raw"
import {
  Bot,
  Check,
  Code,
  Droplet,
  FileText,
  GitBranch,
  Info,
  MonitorPlay,
  Server,
  Shield,
  TerminalSquare,
  X,
} from "lucide-react"
import type { LucideIcon } from "lucide-react"

import { cn } from "@/lib/utils"
import { extractReleaseNotes, parseReleaseNoteLines } from "@/lib/releaseNotes"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import {
  getLanguagePreference,
  setLanguagePreference,
  type LanguagePreference,
} from "@/lib/i18n"
import { useEditorSettingsStore, type EditorFontSize } from "@/state/editorSettingsStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useUiStore } from "@/state/uiStore"
import { useUpdateStore } from "@/state/updateStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { SettingCard, Segmented, ToggleRow } from "./settingsPrimitives"
import { GitSection } from "./GitSection"
import { TerminalSection } from "./TerminalSection"
import { PreviewSection } from "./PreviewSection"
import { AgentSection } from "./AgentSection"
import { LogsSection } from "./LogsSection"
import { LspSection } from "./LspSection"

// Re-export the storage-layer public API so external importers (and tests) keep
// resolving these symbols through this module after the file split.
export {
  TERMINAL_SETTINGS_STORAGE_KEY,
  PREVIEW_SETTINGS_STORAGE_KEY,
  AGENT_SETTINGS_STORAGE_KEY,
  DEFAULT_AGENT_COMMAND,
  loadTerminalSettings,
  loadPreviewSettings,
  loadAgentSettings,
  resolveAgentCommand,
} from "./settingsStorage"
export type {
  AgentPreset,
  TerminalSettings,
  PreviewSettings,
  AgentSettings,
  ThemePreference,
} from "./settingsStorage"

import type { ThemePreference } from "./settingsStorage"

interface SettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  theme: ThemePreference
  onThemeChange: (theme: ThemePreference) => void
  // Optional target applied whenever the dialog opens (or the target changes
  // while open): jump to a section and, for the LSP pane, focus a language card.
  // openSettings("lsp","python") drives these through AppShell (uiStore).
  initialSection?: string
  initialLanguage?: string
  // Bumped by every openSettings call. A dep of the sync effect so re-issuing the
  // SAME target (after the user manually navigated away) still re-applies it —
  // identical section/language primitives alone wouldn't re-fire the effect.
  openNonce?: number
}

function ReleaseNotes({ markdown }: { markdown: string }) {
  return (
    <div className="flex flex-col gap-[7px] text-[11.5px] leading-[1.55] text-(--ink-2)">
      {parseReleaseNoteLines(markdown).map((line, index) => {
        if (line.kind === "heading") {
          return (
            <p key={`${line.kind}-${index}`} className="font-semibold text-(--ink-1)">
              {line.text}
            </p>
          )
        }
        if (line.kind === "item") {
          return (
            <div key={`${line.kind}-${index}`} className="flex gap-[7px]">
              <span aria-hidden="true" className="text-(--yz-accent-ink)">
                •
              </span>
              <span>{line.text}</span>
            </div>
          )
        }
        return <p key={`${line.kind}-${index}`}>{line.text}</p>
      })}
    </div>
  )
}

type SectionId =
  | "appearance"
  | "editor"
  | "safety"
  | "git"
  | "lsp"
  | "agent"
  | "logs"
  | "terminal"
  | "preview"
  | "about"

// Design reference settings nav (§ settingsNav): three panes with icon rows.
// Labels/sub-copy live in the "workbench" i18n namespace under
// settings.sections.<id>.{label,sub} (looked up by id at render time — see
// SettingsDialog below), so this array only carries the id → icon mapping.
const SECTIONS: { id: SectionId; icon: LucideIcon }[] = [
  { id: "appearance", icon: Droplet },
  { id: "editor", icon: Code },
  { id: "lsp", icon: Server },
  { id: "agent", icon: Bot },
  { id: "logs", icon: FileText },
  { id: "terminal", icon: TerminalSquare },
  { id: "preview", icon: MonitorPlay },
  { id: "safety", icon: Shield },
  { id: "git", icon: GitBranch },
  { id: "about", icon: Info },
]

// Reference §2.5 accent table (rgb/solid/ink). Only "lime" is selected here —
// accent switching itself is out of scope (brief §"不在範圍").
const ACCENT_SWATCHES: { id: string; solid: string }[] = [
  { id: "lime", solid: "#86b81f" },
  { id: "blue", solid: "#2f6bff" },
  { id: "violet", solid: "#7b5bff" },
  { id: "coral", solid: "#ff6b54" },
  { id: "amber", solid: "#e0a11f" },
]

/**
 * Settings dialog — design reference settings modal: frost surface, header
 * with avatar, 198px left nav (Appearance / Editor / LSP / Safety / Git +
 * version footer) and a scrollable card pane. Theme + the LSP and Git panes are
 * live; the remaining controls hold local placeholder state until their features
 * land. The dialog remembers the last section across opens, but an external
 * target (initialSection/initialLanguage, from openSettings) overrides it.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  theme,
  onThemeChange,
  initialSection,
  initialLanguage,
  openNonce,
}: SettingsDialogProps) {
  const { t } = useTranslation("common")
  const { t: tw } = useTranslation("workbench")
  const { t: tu } = useTranslation("updates")
  const [section, setSection] = useState<SectionId>("appearance")
  const [targetLanguage, setTargetLanguage] = useState<string | undefined>(undefined)
  const [language, setLanguage] = useState<LanguagePreference>(getLanguagePreference)
  const fontSize = useEditorSettingsStore((s) => s.fontSize)
  const setFontSize = useEditorSettingsStore((s) => s.setFontSize)
  const minimap = useEditorSettingsStore((s) => s.minimap)
  const setMinimap = useEditorSettingsStore((s) => s.setMinimap)
  const moveOpenedWorkspaceToTop = useRecentWorkspacesStore((s) => s.moveOpenedWorkspaceToTop)
  const setMoveOpenedWorkspaceToTop = useRecentWorkspacesStore(
    (s) => s.setMoveOpenedWorkspaceToTop
  )
  const [reconcile, setReconcile] = useState(true)
  const [confirmGit, setConfirmGit] = useState(true)
  const [appVersion, setAppVersion] = useState<string | null>(null)
  const [installBlockedByDirty, setInstallBlockedByDirty] = useState(false)
  const [installConfirmationOpen, setInstallConfirmationOpen] = useState(false)
  const settingsLogSource = useUiStore((s) => s.settingsLogSource)
  const updateStatus = useUpdateStore((s) => s.status)
  const availableUpdate = useUpdateStore((s) => s.update)
  const downloadedBytes = useUpdateStore((s) => s.downloadedBytes)
  const contentLength = useUpdateStore((s) => s.contentLength)
  const checkForUpdates = useUpdateStore((s) => s.checkForUpdates)
  const downloadUpdate = useUpdateStore((s) => s.downloadUpdate)
  const installAndRelaunch = useUpdateStore((s) => s.installAndRelaunch)
  const hasDirtyDocuments = useWorkspaceStore((s) =>
    s.groups.some((group) => group.tabs.some((tab) => tab.dirty))
  )

  useEffect(() => {
    if (!open) return
    let active = true
    void getVersion()
      .then((version) => {
        if (active) setAppVersion(version)
      })
      .catch(() => {
        if (active) setAppVersion(null)
      })
    return () => {
      active = false
    }
  }, [open])

  // Apply an external target on open, and again if the target changes while the
  // dialog stays mounted. `openNonce` (bumped per openSettings) is a dep so
  // re-issuing the SAME target after a manual nav still re-applies it. Manual nav
  // clicks don't touch the props, so they are never fought. A null section leaves
  // the remembered section (rail/palette path).
  useEffect(() => {
    if (!open) return
    const match = SECTIONS.find((s) => s.id === initialSection)
    if (match) setSection(match.id)
    setTargetLanguage(initialLanguage)
  }, [open, initialSection, initialLanguage, openNonce])

  // Manual section nav: switch section and drop any external language highlight
  // (A-F5 — otherwise re-entering the LSP pane keeps the last targeted card lit).
  const selectSection = (id: SectionId) => {
    setSection(id)
    setTargetLanguage(undefined)
  }

  // Persist the display-language choice and switch i18next immediately (live,
  // no reload). "system" follows the OS locale; the two explicit choices pin it.
  const changeLanguage = (pref: LanguagePreference) => {
    setLanguage(pref)
    setLanguagePreference(pref)
  }

  const active = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0]
  const downloadPercent =
    contentLength && contentLength > 0
      ? Math.min(100, Math.round((downloadedBytes / contentLength) * 100))
      : null
  const currentReleaseNotes = appVersion
    ? extractReleaseNotes(changelogMarkdown, appVersion)
    : null

  const requestInstall = () => {
    if (hasDirtyDocuments) {
      setInstallBlockedByDirty(true)
      return
    }
    setInstallBlockedByDirty(false)
    setInstallConfirmationOpen(true)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="yz-diffin flex h-[556px] max-h-[86vh] w-[720px] max-w-[92vw] flex-col gap-0 overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--frost-light) p-0 shadow-(--shadow-xl) ring-0 [backdrop-filter:var(--blur-frost)] sm:max-w-[92vw]"
      >
        <div className="flex shrink-0 items-center gap-[11px] border-b border-(--line-1) px-[20px] py-[15px]">
          <span
            aria-hidden="true"
            className="flex size-[32px] shrink-0 items-center justify-center rounded-full bg-[image:var(--grad-dusk)] text-[13px] font-semibold text-white shadow-(--shadow-xs)"
          >
            Y
          </span>
          <div className="min-w-0 flex-1">
            <DialogTitle className="font-serif text-[18px] leading-[1.1] font-semibold text-(--ink-0)">
              {tw("settings.dialogTitle")}
            </DialogTitle>
            <DialogDescription className="mt-[1px] text-[11px] text-(--ink-3)">
              {tw("settings.dialogDescription")}
            </DialogDescription>
          </div>
          <DialogClose
            aria-label={tw("settings.closeSettings")}
            className="flex size-[28px] shrink-0 items-center justify-center rounded-[8px] text-(--ink-3) transition-colors hover:bg-(--paper-2) hover:text-(--ink-1)"
          >
            <X className="size-[16px]" aria-hidden="true" />
          </DialogClose>
        </div>

        <div className="flex min-h-0 flex-1">
          <aside className="flex w-[198px] shrink-0 flex-col border-r border-(--line-1) bg-(--yz-panel) px-[11px] py-[14px]">
            {SECTIONS.map(({ id, icon: Icon }) => {
              const isActive = id === section
              return (
                <button
                  key={id}
                  type="button"
                  aria-pressed={isActive}
                  onClick={() => selectSection(id)}
                  className={cn(
                    "flex h-[37px] shrink-0 items-center gap-[9px] rounded-[9px] px-[11px] text-[13px] tracking-[-0.01em] transition-all duration-[130ms] ease-(--ease-out)",
                    isActive
                      ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                      : "font-medium text-(--ink-2) hover:bg-(--yz-hover)"
                  )}
                >
                  <span className="flex size-[22px] shrink-0 items-center justify-center">
                    <Icon className="size-[15px]" aria-hidden="true" />
                  </span>
                  <span className="min-w-0 flex-1 text-left">{tw(`settings.sections.${id}.label`)}</span>
                </button>
              )
            })}
            <div className="flex-1" />
            <div className="flex items-center gap-[7px] px-[10px] py-[8px]">
              <span
                aria-hidden="true"
                className="size-[6px] shrink-0 rounded-full bg-(--yz-accent)"
              />
              <span className="font-mono text-[10px] text-(--ink-3)">
                {appVersion
                  ? tw("settings.appVersionValue", { version: appVersion })
                  : tw("settings.appName")}
              </span>
            </div>
          </aside>

          <div className="yzs min-w-0 flex-1 overflow-auto px-[26px] pt-[22px] pb-[26px]">
            <h3 className="font-serif text-[17px] leading-[1.1] font-semibold text-(--ink-0)">
              {tw(`settings.sections.${active.id}.label`)}
            </h3>
            <div className="mt-[3px] mb-[18px] text-[11.5px] text-(--ink-3)">
              {tw(`settings.sections.${active.id}.sub`)}
            </div>

            {section === "appearance" && (
              <div className="flex flex-col gap-[14px]">
                <SettingCard label={tw("settings.theme")}>
                  <Segmented
                    label={tw("settings.theme")}
                    options={[
                      { id: "light", label: tw("settings.themeLight") },
                      { id: "dark", label: tw("settings.themeDark") },
                      { id: "auto", label: tw("settings.themeAuto") },
                    ]}
                    value={theme}
                    onChange={(id) => onThemeChange(id as ThemePreference)}
                  />
                </SettingCard>

                <SettingCard label={tw("settings.accentColor")}>
                  <div
                    role="radiogroup"
                    aria-label={tw("settings.accentColor")}
                    className="flex items-center gap-[11px]"
                  >
                    {ACCENT_SWATCHES.map((swatch) => {
                      const isSelected = swatch.id === "lime"
                      return (
                        <button
                          key={swatch.id}
                          type="button"
                          role="radio"
                          aria-checked={isSelected}
                          aria-label={swatch.id}
                          onClick={() => {
                            /* no-op placeholder — accent switching lands in a later task */
                          }}
                          style={{
                            backgroundColor: swatch.solid,
                            boxShadow: isSelected
                              ? `0 0 0 2px var(--paper-0), 0 0 0 4px ${swatch.solid}`
                              : "var(--shadow-xs)",
                          }}
                          className="flex size-[30px] shrink-0 items-center justify-center rounded-full transition-[transform,box-shadow] duration-150 ease-(--ease-spring) hover:scale-[1.12]"
                        >
                          {isSelected && (
                            <Check
                              className="size-[15px] text-white [&_path]:stroke-[3]"
                              aria-hidden="true"
                            />
                          )}
                        </button>
                      )
                    })}
                  </div>
                </SettingCard>

                <SettingCard label={tw("settings.language")}>
                  <Segmented
                    label={tw("settings.language")}
                    options={[
                      { id: "system", label: t("language.system") },
                      { id: "zh-TW", label: "繁體中文" },
                      { id: "en", label: "English" },
                    ]}
                    value={language}
                    onChange={(id) => changeLanguage(id as LanguagePreference)}
                  />
                </SettingCard>

                <div className="flex flex-col">
                  <ToggleRow
                    label={tw("settings.moveOpenedWorkspaceToTop")}
                    sub={tw("settings.moveOpenedWorkspaceToTopSub")}
                    checked={moveOpenedWorkspaceToTop}
                    onCheckedChange={setMoveOpenedWorkspaceToTop}
                  />
                </div>
              </div>
            )}

            {section === "editor" && (
              <div className="flex flex-col gap-[14px]">
                <SettingCard label={tw("settings.editorFontSize")} sub={tw("settings.editorFontSizeSub")}>
                  <Segmented
                    label={tw("settings.editorFontSize")}
                    options={["12", "13", "14", "15"].map((size) => ({ id: size, label: size }))}
                    value={String(fontSize)}
                    onChange={(id) => setFontSize(Number(id) as EditorFontSize)}
                  />
                </SettingCard>

                {/* Format-on-save + the language-server list are owned by the LSP
                    pane (real, persisted / live) — the editor pane keeps only the
                    editor-surface toggle. */}
                <div className="flex flex-col">
                  <ToggleRow
                    label={tw("settings.showMinimap")}
                    sub={tw("settings.showMinimapSub")}
                    checked={minimap}
                    onCheckedChange={setMinimap}
                  />
                </div>
              </div>
            )}

            {section === "safety" && (
              <div className="flex flex-col">
                <ToggleRow
                  label={tw("settings.reconcileExternalChanges")}
                  sub={tw("settings.reconcileExternalChangesSub")}
                  checked={reconcile}
                  onCheckedChange={setReconcile}
                />
                <ToggleRow
                  label={tw("settings.confirmDestructiveGitActions")}
                  sub={tw("settings.confirmDestructiveGitActionsSub")}
                  locked
                  checked={confirmGit}
                  onCheckedChange={setConfirmGit}
                />
              </div>
            )}

            {section === "lsp" && <LspSection targetLanguage={targetLanguage} />}

            {section === "agent" && <AgentSection />}

            {section === "logs" && (
              <LogsSection initialSource={settingsLogSource ?? undefined} openNonce={openNonce} />
            )}

            {section === "terminal" && <TerminalSection />}

            {section === "preview" && <PreviewSection />}

            {section === "git" && <GitSection />}

            {section === "about" && (
              <div className="flex flex-col gap-[14px]">
                <SettingCard
                  label={tw("settings.currentVersion")}
                  sub={tw("settings.currentVersionSub")}
                >
                  <span className="font-mono text-[13px] font-semibold text-(--ink-1)">
                    {appVersion
                      ? tw("settings.appVersionValue", { version: appVersion })
                      : tw("settings.appName")}
                  </span>
                </SettingCard>
                {currentReleaseNotes && appVersion && (
                  <SettingCard
                    label={tu("currentReleaseNotes")}
                    sub={tu("currentReleaseNotesSub", { version: appVersion })}
                  >
                    <ReleaseNotes markdown={currentReleaseNotes} />
                  </SettingCard>
                )}
                <SettingCard label={tw("settings.updates")} sub={tw("settings.updatesSub")}>
                  <div className="flex min-h-[32px] items-center justify-between gap-[12px]">
                    <span aria-live="polite" className="text-[11.5px] text-(--ink-2)">
                      {updateStatus === "checking" && tw("settings.checkingForUpdates")}
                      {updateStatus === "up-to-date" && tw("settings.upToDate")}
                      {updateStatus === "available" && availableUpdate
                        ? tw("settings.updateAvailable", { version: availableUpdate.version })
                        : null}
                      {updateStatus === "error" && tw("settings.updateCheckFailed")}
                      {updateStatus === "downloading" &&
                        (downloadPercent === null
                          ? tw("settings.downloadingUpdate")
                          : tw("settings.downloadingUpdateProgress", {
                              percent: downloadPercent,
                            }))}
                      {updateStatus === "downloaded" && tw("settings.downloadComplete")}
                      {updateStatus === "download-error" && tw("settings.downloadFailed")}
                      {updateStatus === "installing" && tw("settings.installingUpdate")}
                      {updateStatus === "install-error" && tw("settings.installFailed")}
                    </span>
                    {updateStatus === "available" ? (
                      <Button type="button" size="sm" onClick={() => void downloadUpdate()}>
                        {tw("settings.downloadUpdate")}
                      </Button>
                    ) : updateStatus === "download-error" ? (
                      <Button type="button" size="sm" onClick={() => void downloadUpdate()}>
                        {tw("settings.retryDownload")}
                      </Button>
                    ) : updateStatus === "downloading" ? (
                      <Button type="button" size="sm" disabled>
                        {tw("settings.downloadingUpdate")}
                      </Button>
                    ) : updateStatus === "downloaded" ? (
                      <Button type="button" size="sm" onClick={requestInstall}>
                        {tw("settings.installAndRestart")}
                      </Button>
                    ) : updateStatus === "installing" ? (
                      <Button type="button" size="sm" disabled>
                        {tw("settings.installingUpdate")}
                      </Button>
                    ) : updateStatus === "install-error" ? (
                      <Button type="button" size="sm" onClick={requestInstall}>
                        {tw("settings.retryInstall")}
                      </Button>
                    ) : (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={updateStatus === "checking"}
                        onClick={() => void checkForUpdates()}
                      >
                        {updateStatus === "checking"
                          ? tw("settings.checkingForUpdates")
                          : updateStatus === "error"
                            ? tw("settings.retryUpdateCheck")
                            : updateStatus === "up-to-date"
                              ? tw("settings.checkAgain")
                              : tw("settings.checkForUpdates")}
                      </Button>
                    )}
                  </div>
                  {updateStatus === "downloading" && downloadPercent !== null && (
                    <div
                      role="progressbar"
                      aria-label={tw("settings.downloadProgress")}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={downloadPercent}
                      className="mt-[8px] h-[5px] overflow-hidden rounded-full bg-(--paper-2)"
                    >
                      <div
                        className="h-full rounded-full bg-(--yz-accent) transition-[width]"
                        style={{ width: `${downloadPercent}%` }}
                      />
                    </div>
                  )}
                  {availableUpdate?.body?.trim() && (
                    <div className="mt-[12px] border-t border-(--line-1) pt-[11px]">
                      <p className="mb-[8px] text-[11.5px] font-semibold text-(--ink-1)">
                        {tu("availableReleaseNotes", {
                          version: availableUpdate.version,
                        })}
                      </p>
                      <ReleaseNotes markdown={availableUpdate.body} />
                    </div>
                  )}
                  {installBlockedByDirty && updateStatus === "downloaded" && (
                    <p role="alert" className="mt-[9px] text-[11px] text-destructive">
                      {tw("settings.unsavedDocumentsBlockInstall")}
                    </p>
                  )}
                </SettingCard>
              </div>
            )}
          </div>
        </div>
      </DialogContent>
      <Dialog open={installConfirmationOpen} onOpenChange={setInstallConfirmationOpen}>
        <DialogContent showCloseButton={false} className="sm:max-w-[440px]">
          <DialogHeader>
            <DialogTitle>{tw("settings.installConfirmTitle")}</DialogTitle>
            <DialogDescription>{tw("settings.installConfirmDescription")}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setInstallConfirmationOpen(false)}>
              {tw("settings.cancelInstall")}
            </Button>
            <Button
              onClick={() => {
                setInstallConfirmationOpen(false)
                void installAndRelaunch()
              }}
            >
              {tw("settings.installAndRestart")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Dialog>
  )
}
