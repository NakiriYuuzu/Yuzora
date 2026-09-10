import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { getVersion } from "@tauri-apps/api/app"
import changelogMarkdown from "../../../CHANGELOG.md?raw"
import {
  Check,
  Search,
  ArrowRight,
  Code,
  Droplet,
  FileText,
  GitBranch,
  Info,
  Shield,
  TerminalSquare,
  Bot,
  X,
} from "lucide-react"
import type { LucideIcon } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Tabs,TabsList,TabsTrigger,TabsContent } from "@/components/ui/tabs"
import { FieldGroup } from "@/components/ui/field"
import { InputGroup,InputGroupInput,InputGroupAddon,InputGroupButton } from "@/components/ui/input-group"
import { Empty,EmptyHeader,EmptyTitle,EmptyDescription,EmptyContent } from "@/components/ui/empty"
import { SettingsThemePicker } from "./SettingsThemePicker"
import { SETTINGS_GROUPS,settingsSearchResults,type SettingsSectionId } from "./settings-search"
import "./settings-modern.css"
import { extractReleaseNotes, parseReleaseNoteLines } from "@/lib/releaseNotes"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  dialogMinSize,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { ScrollArea } from "@/components/ui/scroll-area"
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
import { workspacePathForDisplay } from "@/lib/paths"
import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"
import { SettingCard, Segmented, ToggleRow } from "./settingsPrimitives"
import { BrandMark } from "@/components/BrandMark"
import { HerdrSettingsSection } from "@/app/workbench/HerdrSettingsSection"
import { GitSection } from "./GitSection"
import { TerminalSection } from "./TerminalSection"
import { LogsSection } from "./LogsSection"

// Re-export the storage-layer public API so external importers (and tests) keep
// resolving these symbols through this module after the file split.
export {
  TERMINAL_SETTINGS_STORAGE_KEY,
} from "./settingsStorage"
export type {
  ThemePreference,
} from "./settingsStorage"

import type { ThemePreference } from "./settingsStorage"
import type { TrustedWorkspace } from "@/lib/types"
import {
  ACCENT_THEMES,
  DEFAULT_ACCENT_PREFERENCE,
  type AccentPreference,
} from "@/theme/accent"

interface SettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  theme: ThemePreference
  onThemeChange: (theme: ThemePreference) => void
  accent?: AccentPreference
  onAccentChange?: (accent: AccentPreference) => void
  leftSidebarBackground?: boolean
  rightSidebarBackground?: boolean
  onSidebarBackgroundChange?: (side: "left" | "right", enabled: boolean) => void
  // Optional target applied whenever the dialog opens (or the target changes
  // while open).
  initialSection?: string
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

type SectionId = SettingsSectionId

// Settings category icons; grouping is owned by SETTINGS_GROUPS.
// Labels/sub-copy live in the "workbench" i18n namespace under
// settings.sections.<id>.{label,sub} (looked up by id at render time — see
// SettingsDialog below), so this array only carries the id → icon mapping.
const SECTIONS: { id: SectionId; icon: LucideIcon }[] = [
  { id: "appearance", icon: Droplet },
  { id: "editor", icon: Code },
  { id: "logs", icon: FileText },
  { id: "terminal", icon: TerminalSquare },
  { id: "herdr", icon: Bot },
  { id: "safety", icon: Shield },
  { id: "git", icon: GitBranch },
  { id: "about", icon: Info },
]

const ACCENT_SWATCHES = Object.entries(ACCENT_THEMES).map(([id, palette]) => ({
  id: id as AccentPreference,
  solid: palette.solid,
}))

/**
 * Settings: grouped navigation, search and a scrollable settings pane.
 * Existing stores and native services retain their original behavior.
 * The dialog remembers the last section across opens, but an external
 * target (initialSection, from openSettings) overrides it.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  theme,
  onThemeChange,
  accent = DEFAULT_ACCENT_PREFERENCE,
  onAccentChange = () => {},
  leftSidebarBackground = true,
  rightSidebarBackground = true,
  onSidebarBackgroundChange = () => {},
  initialSection,
  openNonce,
}: SettingsDialogProps) {
  const { t } = useTranslation("common")
  const { t: tw } = useTranslation("workbench")
  const { t: tu } = useTranslation("updates")
  const { t: td } = useTranslation("settingsDemo")
  const [query,setQuery] = useState("")
  const [jumpTarget,setJumpTarget] = useState<string|null>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const previousFocusRef = useRef<HTMLElement|null>(null)
  const searchResults = query.trim() ? settingsSearchResults(query,tw) : []
  const [section, setSection] = useState<SectionId>("appearance")
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
  const trustedWorkspaces = useWorkspaceTrustStore((s) => s.trustedWorkspaces)
  const refreshTrustList = useWorkspaceTrustStore((s) => s.refreshList)
  const revokeWorkspace = useWorkspaceTrustStore((s) => s.revokeWorkspace)

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
    setQuery("")
    setJumpTarget(null)
    const match = SECTIONS.find((s) => s.id === initialSection)
    if (match) setSection(match.id)
  }, [open, initialSection, openNonce])

  // Manual section nav clears the current search target.
  const selectSection = (id: SectionId) => {
    setSection(id)
    setQuery("")
    setJumpTarget(null)
  }

  useEffect(()=>{
    if(!open || query || !jumpTarget)return
    const target=Array.from(contentRef.current?.querySelectorAll<HTMLElement>("[data-settings-label]")??[]).find(item=>item.dataset.settingsLabel===jumpTarget)
    if(!target){contentRef.current?.focus();return}
    target.scrollIntoView?.({block:"center"})
    const control=target.querySelector<HTMLElement>('[aria-checked="true"]')??target.querySelector<HTMLElement>('button,input,select,textarea,[tabindex="0"]')
    if(control)control.focus({preventScroll:true})
    else contentRef.current?.focus({preventScroll:true})
  },[open,section,query,jumpTarget])
  function openSearchResult(result:typeof searchResults[number]) {
    setSection(result.section);setQuery("");setJumpTarget(result.target)
    if(!result.target)requestAnimationFrame(()=>contentRef.current?.focus())
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
        resizeId="settings"
        minSize={dialogMinSize(640, 440)}
        showCloseButton={false}
        className="settings-modern"
        data-design="settings"
        data-design-label={td("title")}
        onOpenAutoFocus={event=>{event.preventDefault();previousFocusRef.current=document.activeElement as HTMLElement;searchRef.current?.focus()}}
        onCloseAutoFocus={event=>{event.preventDefault();previousFocusRef.current?.focus()}}
        onEscapeKeyDown={event=>{if(query){event.preventDefault();setQuery('');searchRef.current?.focus()}}}
        onKeyDownCapture={event=>{if((event.metaKey||event.ctrlKey)&&event.key.toLowerCase()==="f"){event.preventDefault();event.stopPropagation();searchRef.current?.focus();searchRef.current?.select()}}}
      >
        <header className="settings-topbar">
          <div className="settings-heading"><DialogTitle>{td("title")}</DialogTitle><DialogDescription>{td("subtitle")}</DialogDescription></div>
          <InputGroup className="settings-search" data-design="settings-search" data-design-label={td("search")}>
            <InputGroupAddon><Search aria-hidden="true"/></InputGroupAddon>
            <InputGroupInput ref={searchRef} aria-label={td("search")} placeholder={td("searchPlaceholder")} value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{
              if(event.key==='Enter'&&searchResults[0]&&!event.nativeEvent.isComposing){event.preventDefault();openSearchResult(searchResults[0])}
            }}/>
            {query&&<InputGroupAddon align="inline-end"><InputGroupButton aria-label={td("clearSearch")} size="icon-xs" onClick={()=>{setQuery('');searchRef.current?.focus()}}><X aria-hidden="true"/></InputGroupButton></InputGroupAddon>}
          </InputGroup>
          <DialogClose asChild><Button variant="ghost" size="icon-sm" aria-label={tw("settings.closeSettings")}><X aria-hidden="true"/></Button></DialogClose>
        </header>
        <Tabs orientation="vertical" value={section} onValueChange={value=>selectSection(value as SectionId)} className="settings-layout">
          <aside data-testid="settings-sidebar" className="settings-sidebar" data-design="settings-navigation" data-design-label={td("categories")}>
            <div className="settings-app-identity"><BrandMark /><div><strong>Yuzora</strong><small>{td("localPreferences")}</small></div></div>
            <ScrollArea data-testid="settings-sidebar-scroll" className="settings-nav-scroll min-h-0 flex-1">
              <TabsList aria-label={td("categories")} className="settings-nav-list">
                {SETTINGS_GROUPS.map(group=><div key={group.id} className="settings-nav-group">
                  <span className="settings-nav-group-label">{td(`groups.${group.id}`)}</span>
                  {group.sections.map(id=>{const Icon=SECTIONS.find(item=>item.id===id)!.icon;return <TabsTrigger key={id} value={id}><Icon aria-hidden="true"/><span>{tw(`settings.sections.${id}.label`)}</span></TabsTrigger>})}
                </div>)}
              </TabsList>
            </ScrollArea>
            <div data-testid="settings-sidebar-footer" className="settings-sidebar-footer shrink-0"><Badge variant="outline">Yuzora</Badge><span>{appVersion ? tw("settings.appVersionValue", { version: appVersion }) : tw("settings.appName")}</span></div>
          </aside>
          <div className="settings-main">
            <ScrollArea key={query.trim()?'search':section} className="settings-content-scroll" viewportClassName="settings-content-viewport" focusable>
              {query.trim()?<div className="settings-search-results">
                <div className="settings-page-heading"><h3>{td("resultsTitle")}</h3><p role="status">{td("resultsCount",{count:searchResults.length,query})}</p></div>
                {searchResults.length?<div className="settings-result-list">{searchResults.map(result=><Button key={result.key} variant="ghost" className="settings-result" onClick={()=>openSearchResult(result)}><span><small>{result.category}</small><strong>{result.label}</strong></span><ArrowRight aria-hidden="true"/></Button>)}</div>:<Empty><EmptyHeader><EmptyTitle>{td("noResults")}</EmptyTitle><EmptyDescription>{td("noResultsHint")}</EmptyDescription></EmptyHeader><EmptyContent><Button variant="outline" onClick={()=>{setQuery('');searchRef.current?.focus()}}>{td("clearSearch")}</Button></EmptyContent></Empty>}
              </div>:<TabsContent value={section} ref={contentRef} className="settings-page" tabIndex={0} data-design="settings-content" data-design-label={tw(`settings.sections.${section}.label`)}>
                <div className="settings-page-heading"><span className="settings-page-eyebrow">{td(`groups.${SETTINGS_GROUPS.find(group=>group.sections.some(id=>id===section))!.id}`)}</span><h3>{tw(`settings.sections.${active.id}.label`)}</h3><p>{tw(`settings.sections.${active.id}.sub`)}</p></div>

            {section === "appearance" && (
              <FieldGroup className="settings-fields">
                <SettingCard label={tw("settings.theme")}>
                  <SettingsThemePicker value={theme} onChange={onThemeChange}/>
                  <p className="settings-inline-hint">{td("themeHint")}</p>
                </SettingCard>

                <SettingCard label={tw("settings.accentColor")}>
                  <RadioGroup aria-label={tw("settings.accentColor")} value={accent} onValueChange={value=>onAccentChange(value as AccentPreference)} className="settings-palette">
                    {ACCENT_SWATCHES.map(swatch=><label key={swatch.id} className="settings-palette-choice" data-selected={accent===swatch.id}>
                      <RadioGroupItem value={swatch.id} aria-label={td(`palettes.${swatch.id}`)} style={{backgroundColor:swatch.solid}} className="settings-palette-swatch"><Check aria-hidden="true"/></RadioGroupItem>
                      <span>{td(`palettes.${swatch.id}`)}</span>
                    </label>)}
                  </RadioGroup>
                  <p className="settings-inline-hint">{td("paletteHint")}</p>
                </SettingCard>

                <div className="flex flex-col">
                  <ToggleRow
                    label={tw("settings.leftSidebarBackground")}
                    sub={tw("settings.leftSidebarBackgroundSub")}
                    checked={leftSidebarBackground}
                    onCheckedChange={enabled => onSidebarBackgroundChange("left", enabled)}
                  />
                  <ToggleRow
                    label={tw("settings.rightSidebarBackground")}
                    sub={tw("settings.rightSidebarBackgroundSub")}
                    checked={rightSidebarBackground}
                    onCheckedChange={enabled => onSidebarBackgroundChange("right", enabled)}
                  />
                </div>

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
              </FieldGroup>
            )}

            {section === "editor" && (
              <FieldGroup className="settings-fields">
                <SettingCard label={tw("settings.editorFontSize")} sub={tw("settings.editorFontSizeSub")}>
                  <Segmented
                    label={tw("settings.editorFontSize")}
                    options={["12", "13", "14", "15"].map((size) => ({ id: size, label: size }))}
                    value={String(fontSize)}
                    onChange={(id) => setFontSize(Number(id) as EditorFontSize)}
                  />
                </SettingCard>

                <div className="settings-editor-preview" aria-label={td("editorPreview")} style={{fontSize}}><div><span>workspace.ts</span><span>{fontSize}px · JetBrains Mono</span></div><pre><code><span>1  </span>const workspace = "Yuzora";{"\n"}<span>2  </span>// {td("editorSample")}{"\n"}<span>3  </span>await agent.read();</code></pre></div>

                <div className="flex flex-col">
                  <ToggleRow
                    label={tw("settings.showMinimap")}
                    sub={tw("settings.showMinimapSub")}
                    checked={minimap}
                    onCheckedChange={setMinimap}
                  />
                </div>
              </FieldGroup>
            )}

            {section === "safety" && (
              <SafetySettingsSection
                reconcile={reconcile}
                onReconcileChange={setReconcile}
                confirmGit={confirmGit}
                onConfirmGitChange={setConfirmGit}
                open={open}
                trustedWorkspaces={trustedWorkspaces}
                onRefreshTrustList={refreshTrustList}
                onRevokeWorkspace={revokeWorkspace}
              />
            )}

            {section === "logs" && (
              <LogsSection initialSource={settingsLogSource ?? undefined} openNonce={openNonce} />
            )}

            {section === "terminal" && <TerminalSection />}

            {section === "herdr" && <HerdrSettingsSection />}

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
              </TabsContent>}
            </ScrollArea>
            <footer className="settings-content-footer"><Info aria-hidden="true"/><span>{td("preferencesHint")}</span></footer>
          </div>
        </Tabs>
      </DialogContent>
      <Dialog open={installConfirmationOpen} onOpenChange={setInstallConfirmationOpen}>
<DialogContent
          resizeId="settings-install"
          showCloseButton={false}
          className="flex min-h-0 flex-col"
        >
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

function SafetySettingsSection({
  reconcile,
  onReconcileChange,
  confirmGit,
  onConfirmGitChange,
  open,
  trustedWorkspaces,
  onRefreshTrustList,
  onRevokeWorkspace,
}: {
  reconcile: boolean
  onReconcileChange: (value: boolean) => void
  confirmGit: boolean
  onConfirmGitChange: (value: boolean) => void
  open: boolean
  trustedWorkspaces: TrustedWorkspace[]
  onRefreshTrustList: () => Promise<TrustedWorkspace[]>
  onRevokeWorkspace: (canonicalPath: string) => Promise<TrustedWorkspace[]>
}) {
  const { t: tw } = useTranslation("workbench")

  useEffect(() => {
    if (!open) return
    void onRefreshTrustList().catch(() => {
      // List errors stay in the trust store.
    })
  }, [open, onRefreshTrustList])

  return (
    <div className="flex flex-col gap-[14px]">
      <ToggleRow
        label={tw("settings.reconcileExternalChanges")}
        sub={tw("settings.reconcileExternalChangesSub")}
        checked={reconcile}
        onCheckedChange={onReconcileChange}
      />
      <ToggleRow
        label={tw("settings.confirmDestructiveGitActions")}
        sub={tw("settings.confirmDestructiveGitActionsSub")}
        locked
        checked={confirmGit}
        onCheckedChange={onConfirmGitChange}
      />
      <SettingCard
        label={tw("settings.trustedWorkspaces")}
        sub={tw("settings.trustedWorkspacesSub")}
      >
        {trustedWorkspaces.length === 0 ? (
          <p className="text-[12px] text-(--ink-3)">{tw("settings.noTrustedWorkspaces")}</p>
        ) : (
          <ScrollArea className="max-h-[220px]">
            <ul className="flex flex-col gap-[8px] pr-[4px]">
              {trustedWorkspaces.map((workspace) => (
                <li
                  key={workspace.canonicalPath}
                  className="flex items-start justify-between gap-[12px] rounded-[8px] border border-(--line-1) bg-(--yz-sunk) px-[10px] py-[8px]"
                >
                  <div className="min-w-0">
                    <p className="break-all font-mono text-[12px] text-(--ink-1)">
                      {workspacePathForDisplay(workspace.canonicalPath)}
                    </p>
                    <p className="mt-[2px] text-[11px] text-(--ink-3)">
                      {tw("settings.trustedWorkspaceGrantedAt", {
                        date: workspace.grantedAt,
                      })}
                    </p>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-[26px] shrink-0 px-[8px] text-[11px]"
                    aria-label={tw("settings.revokeWorkspaceNamed", {
                      path: workspacePathForDisplay(workspace.canonicalPath),
                    })}
                    onClick={() => void onRevokeWorkspace(workspace.canonicalPath)}
                  >
                    {tw("settings.revokeWorkspace")}
                  </Button>
                </li>
              ))}
            </ul>
          </ScrollArea>
        )}
      </SettingCard>
    </div>
  )
}
