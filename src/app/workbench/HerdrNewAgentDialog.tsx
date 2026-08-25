import { BotIcon, RotateCwIcon } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/command"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog"
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel
} from "@/components/ui/field"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Switch } from "@/components/ui/switch"
import { herdrAgentCatalog } from "@/lib/herdrIpc"
import { openCreatedHerdrTab } from "@/lib/herdrTabActions"
import type { HerdrAgentCatalogEntry } from "@/lib/herdrTypes"
import { workspacePathForDisplay } from "@/lib/paths"
import { useHerdrStore } from "@/state/herdrStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useUiStore } from "@/state/uiStore"

interface HerdrNewAgentDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function HerdrNewAgentDialog({
  open,
  onOpenChange
}: HerdrNewAgentDialogProps) {
  const { t } = useTranslation("workbench")
  useOverlayPresence(open)
  const selectedSessionName = useHerdrStore((state) => state.selectedSessionName)
  const selectedSpaceId = useHerdrStore((state) => state.selectedSpaceId)
  const snapshot = useHerdrStore((state) => state.snapshot)
  const selectedSpace = useMemo(
    () => snapshot?.spaces.find((space) => space.id === selectedSpaceId) ?? null,
    [selectedSpaceId, snapshot?.spaces]
  )
  const createAgent = useHerdrStore((state) => state.createAgentInSelectedSpace)
  const createBlockedReason = useHerdrStore((state) => state.createAgentBlockedReason())
  const setMode = useUiStore((state) => state.setMode)
  const [catalog, setCatalog] = useState<HerdrAgentCatalogEntry[]>([])
  const [selectedKind, setSelectedKind] = useState("")
  const [bypassPermissions, setBypassPermissions] = useState(false)
  const [loading, setLoading] = useState(false)
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const catalogGeneration = useRef(0)
  const selectedEntry = useMemo(
    () => catalog.find((entry) => entry.agent === selectedKind) ?? null,
    [catalog, selectedKind]
  )

  const loadCatalog = useCallback(() => {
    if (!selectedSessionName) return
    const generation = ++catalogGeneration.current
    setLoading(true)
    setError(null)
    void herdrAgentCatalog(selectedSessionName)
      .then((entries) => {
        if (generation !== catalogGeneration.current) return
        const sorted = [...entries].sort((left, right) => {
          const detected = Number(Boolean(right.detectedBinaryPath)) - Number(Boolean(left.detectedBinaryPath))
          if (detected !== 0) return detected
          return left.agent.localeCompare(right.agent, undefined, {
            numeric: true,
            sensitivity: "base"
          })
        })
        setCatalog(sorted)
        setBypassPermissions(false)
        setSelectedKind((current) =>
          sorted.some((entry) => entry.agent === current)
            ? current
            : (sorted[0]?.agent ?? "")
        )
      })
      .catch((caught) => {
        if (generation !== catalogGeneration.current) return
        setCatalog([])
        setSelectedKind("")
        setError(caught instanceof Error ? caught.message : String(caught))
      })
      .finally(() => {
        if (generation === catalogGeneration.current) setLoading(false)
      })
  }, [selectedSessionName])

  useEffect(() => {
    if (!open) return
    let active = true
    void Promise.resolve().then(() => {
      if (active) loadCatalog()
    })
    return () => {
      active = false
      catalogGeneration.current += 1
    }
  }, [loadCatalog, open])

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) setBypassPermissions(false)
    onOpenChange(nextOpen)
  }

  const onStart = async () => {
    if (!selectedKind || creating || !selectedSpaceId) return
    setCreating(true)
    setError(null)
    try {
      const created = await createAgent(selectedKind, bypassPermissions)
      if (!created) {
        setError(
          useHerdrStore.getState().errorMessage ??
            createBlockedReason ??
            t("herdrNav.createAgentFailedUnknown")
        )
        return
      }
      setMode("ade")
      await openCreatedHerdrTab({
        sessionName: created.herdrSessionId,
        workspaceId: created.workspaceId,
        terminalId: created.terminalId,
        paneId: created.paneId,
        tabId: created.tabId,
        title: created.title ?? created.kind
      })
      handleOpenChange(false)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setCreating(false)
    }
  }

  const canStart = Boolean(
    selectedSessionName &&
      selectedSpaceId &&
      selectedKind &&
      !loading &&
      !creating &&
      !createBlockedReason
  )

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[560px]">
        <DialogHeader>
          <DialogTitle>{t("herdrNav.newAgent")}</DialogTitle>
          <DialogDescription>
            {t("herdrNav.newAgentDescription", {
              space: selectedSpace?.label ?? selectedSpaceId ?? "—"
            })}
          </DialogDescription>
        </DialogHeader>

        <FieldGroup>
          <Field>
            <Command value={selectedKind} onValueChange={setSelectedKind}>
              <CommandInput placeholder={t("herdrNav.searchAgents")} />
              <ScrollArea className="h-[240px]">
                <CommandList>
                  <CommandEmpty>
                    {loading ? t("herdrNav.loadingAgentCatalog") : t("herdrNav.noAgentCatalog")}
                  </CommandEmpty>
                  <CommandGroup heading={t("herdrNav.agentCatalogHeading")}>
                    {catalog.map((entry) => (
                      <CommandItem
                        key={entry.agent}
                        value={entry.agent}
                        data-checked={selectedKind === entry.agent}
                        onSelect={() => {
                          setSelectedKind(entry.agent)
                          setBypassPermissions(false)
                        }}
                      >
                        <BotIcon aria-hidden="true" />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate font-medium">{entry.agent}</span>
                          <span className="block truncate text-xs text-muted-foreground">
                            {entry.detectedBinaryPath
                              ? workspacePathForDisplay(entry.detectedBinaryPath)
                              : t("herdrNav.agentPathServerValidated")}
                          </span>
                        </span>
                        {entry.activeVersion && (
                          <span className="shrink-0 font-mono text-xs text-muted-foreground">
                            {entry.activeVersion}
                          </span>
                        )}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                </CommandList>
              </ScrollArea>
            </Command>
          </Field>

          {selectedEntry && selectedEntry.bypassFlags.length > 0 && (
            <Field orientation="horizontal">
              <FieldContent>
                <FieldLabel htmlFor="herdr-agent-bypass">
                  {t("herdrNav.bypassPermissions")}
                </FieldLabel>
                <FieldDescription>
                  {t("herdrNav.bypassPermissionsDescription", {
                    flags: selectedEntry.bypassFlags.join(" ")
                  })}
                </FieldDescription>
              </FieldContent>
              <Switch
                id="herdr-agent-bypass"
                checked={bypassPermissions}
                onCheckedChange={setBypassPermissions}
                disabled={creating}
              />
            </Field>
          )}

          {createBlockedReason && !error && (
            <FieldError>{createBlockedReason}</FieldError>
          )}
          {error && <FieldError>{error}</FieldError>}
        </FieldGroup>

        <DialogFooter>
          <DialogClose asChild>
            <Button type="button" variant="outline" disabled={creating}>
              {t("herdrNav.cancel")}
            </Button>
          </DialogClose>
          {error && catalog.length === 0 && (
            <Button type="button" variant="outline" disabled={loading} onClick={loadCatalog}>
              <RotateCwIcon data-icon="inline-start" aria-hidden="true" />
              {t("herdrNav.retryAgentCatalog")}
            </Button>
          )}
          <Button type="button" disabled={!canStart} onClick={() => void onStart()}>
            <BotIcon data-icon="inline-start" aria-hidden="true" />
            {creating ? t("herdrNav.startingAgent") : t("herdrNav.startAgent")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
