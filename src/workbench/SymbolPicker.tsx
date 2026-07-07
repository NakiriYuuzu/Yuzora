import { useEffect, useState } from "react"
import { Command as CommandPrimitive } from "cmdk"
import { SearchIcon } from "lucide-react"
import { useTranslation } from "react-i18next"

import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from "../components/ui/command"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../components/ui/dialog"
import { ensureClient } from "../lsp/lspManager"
import { requestDocumentSymbols, requestWorkspaceSymbols } from "../lsp/symbols"
import type { FlatSymbol, WorkspaceSymbolItem } from "../lsp/symbols"
import { pathToUri, uriToPath } from "../lsp/workspace"
import { getDocument } from "../editor/documentRegistry"
import { fileGradeOf, lspLanguageOf } from "../lib/types"
import { useOverlayPresence } from "../state/overlayStore"
import { useWorkspaceStore } from "../state/workspaceStore"

interface SymbolPickerProps {
    open: boolean
    onOpenChange: (open: boolean) => void
    mode: "document" | "workspace"
}

// Minimal structural view of the two symbol capabilities. lspManager's
// ServerCapabilities mirror only declares documentFormattingProvider, but the
// runtime value is the server's full capabilities; cast to read the two flags
// without editing that shared mirror (T10 scope).
interface SymbolCapabilities {
    documentSymbolProvider?: unknown
    workspaceSymbolProvider?: unknown
}

const DEBOUNCE_MS = 250

// Resolve the managed client for the current file's language. Workspace symbols
// also route through the active file's language server (there is one server per
// language). Returns nulls when there is no open workspace / active file / a
// language we don't drive an LSP for. `gradeGate` toggles the file-grade check
// (document mode only — see below).
async function resolveActive(gradeGate: boolean) {
    const { workspacePath, groups, activeGroupIndex } = useWorkspaceStore.getState()
    const activePath = groups[activeGroupIndex]?.activePath ?? null
    if (!workspacePath || !activePath) return { managed: null, activePath: null }
    const language = lspLanguageOf(activePath)
    if (!language) return { managed: null, activePath }
    // Document mode gates on the active file's grade the same way EditorPane does:
    // a non-full file (tooLarge/binary/limited/nonUtf8/very-long-line) must not
    // spin up a server the grade rejects nor query a URI that was never didOpen'd.
    // This reasoning does NOT hold for workspace/symbol — a cross-file query that
    // never touches the active URI — so that mode skips the gate (an oversized
    // active file must not disable workspace search). getDocument may reject (the
    // active file was deleted / became unreadable between opening the picker and
    // this resolve) → fall back to the empty state rather than letting the effect's
    // async IIFE reject unhandled.
    if (gradeGate) {
        const entry = await getDocument(activePath).catch(() => null)
        if (!entry || entry.result.kind === "tooLarge" || entry.result.kind === "binary") {
            return { managed: null, activePath }
        }
        if (fileGradeOf(entry.result, entry.result.content) !== "full") {
            return { managed: null, activePath }
        }
    }
    const managed = await ensureClient(workspacePath, language)
    return { managed, activePath }
}

/**
 * Symbol picker — go-to-symbol (document outline) and workspace symbols, driven
 * by the active file's language server (T10). Self-sufficient: reads the
 * workspace store directly, so CommandPalette only owns open/mode. Capability
 * downgrade shows an empty "no symbols" list rather than erroring.
 */
export function SymbolPicker({ open, onOpenChange, mode }: SymbolPickerProps) {
    const { t } = useTranslation("lsp")
    const [query, setQuery] = useState("")
    const [debouncedQuery, setDebouncedQuery] = useState("")
    const [docSymbols, setDocSymbols] = useState<FlatSymbol[]>([])
    const [wsSymbols, setWsSymbols] = useState<WorkspaceSymbolItem[]>([])

    // Register with the central z-order gate so the preview child webview hides
    // while this picker is open (same as CommandPalette / BranchPopover).
    useOverlayPresence(open)

    // Reset transient state whenever the picker closes so a reopen starts clean.
    useEffect(() => {
        if (open) return
        setQuery("")
        setDebouncedQuery("")
        setDocSymbols([])
        setWsSymbols([])
    }, [open])

    // Document outline: load once on open (no debounce — it's a single request
    // for the already-open file, filtered client-side by the input below).
    useEffect(() => {
        if (!open || mode !== "document") return
        let cancelled = false
        void (async () => {
            const { managed, activePath } = await resolveActive(true)
            if (cancelled) return
            // Capabilities are populated only after the server's initialize
            // handshake resolves — lspManager fills managed.capabilities in a
            // fire-and-forget .then on this same promise (registered first). Await
            // it so a cold server (rust-analyzer can take seconds) isn't misread as
            // "no provider" → empty; [open, mode] wouldn't re-run once caps arrive.
            if (managed) await managed.client.initializing.catch(() => {})
            if (cancelled) return
            const caps = managed?.capabilities as SymbolCapabilities | null
            if (!managed || !activePath || !caps?.documentSymbolProvider) {
                setDocSymbols([])
                return
            }
            // A per-request failure (server error / timeout / disconnect) isn't
            // caught by the capability gate — degrade to the empty list (same as a
            // missing provider) rather than letting the IIFE reject unhandled.
            const symbols = await requestDocumentSymbols(managed.client, pathToUri(activePath)).catch(
                () => [] as FlatSymbol[]
            )
            if (!cancelled) setDocSymbols(symbols)
        })()
        return () => {
            cancelled = true
        }
    }, [open, mode])

    // Debounce the raw input into the query the server sees (workspace mode).
    useEffect(() => {
        const id = setTimeout(() => setDebouncedQuery(query), DEBOUNCE_MS)
        return () => clearTimeout(id)
    }, [query])

    // Workspace symbols: server-side search, re-run on each debounced query.
    useEffect(() => {
        if (!open || mode !== "workspace" || !debouncedQuery.trim()) {
            setWsSymbols([])
            return
        }
        let cancelled = false
        void (async () => {
            const { managed } = await resolveActive(false)
            if (cancelled) return
            // Wait for the initialize handshake before reading capabilities (see
            // the document effect) so a cold server isn't misread as no provider.
            if (managed) await managed.client.initializing.catch(() => {})
            if (cancelled) return
            const caps = managed?.capabilities as SymbolCapabilities | null
            if (!managed || !caps?.workspaceSymbolProvider) {
                setWsSymbols([])
                return
            }
            // Degrade a per-request failure to the empty list (see document effect).
            const symbols = await requestWorkspaceSymbols(managed.client, debouncedQuery).catch(
                () => [] as WorkspaceSymbolItem[]
            )
            if (!cancelled) setWsSymbols(symbols)
        })()
        return () => {
            cancelled = true
        }
    }, [open, mode, debouncedQuery])

    const selectDocument = (symbol: FlatSymbol) => {
        const activePath = useWorkspaceStore.getState().groups[
            useWorkspaceStore.getState().activeGroupIndex
        ]?.activePath
        if (activePath) {
            // 1-based line; requestReveal opens the tab and EditorPane reveals + focuses.
            useWorkspaceStore.getState().requestReveal(activePath, symbol.range.start.line + 1)
        }
        onOpenChange(false)
    }

    const selectWorkspace = (item: WorkspaceSymbolItem) => {
        const path = uriToPath(item.uri)
        // requestReveal opens the tab (and EditorPane reveals + focuses); no need
        // to openTab first — it calls get().openTab(path) internally.
        useWorkspaceStore.getState().requestReveal(path, item.range.start.line + 1)
        onOpenChange(false)
    }

    // Document mode filters the loaded outline locally; workspace mode leaves
    // filtering to the server (results are already query-scoped).
    const filteredDoc = query
        ? docSymbols.filter((s) => s.name.toLowerCase().includes(query.toLowerCase()))
        : docSymbols

    const placeholder =
        mode === "document" ? t("goToSymbolPlaceholder") : t("workspaceSymbolsPlaceholder")

    return (
        <Dialog open={open} onOpenChange={onOpenChange}>
            <DialogHeader className="sr-only">
                <DialogTitle>{t("symbolPickerTitle")}</DialogTitle>
                <DialogDescription>{placeholder}</DialogDescription>
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
                            value={query}
                            onValueChange={setQuery}
                            placeholder={placeholder}
                            className="flex-1 bg-transparent text-[19px] font-medium text-(--ink-1) outline-none placeholder:text-(--ink-3)"
                        />
                        <kbd className="shrink-0 rounded-[6px] bg-(--yz-active) px-[8px] py-[3px] font-mono text-[11px] text-(--ink-3)">
                            esc
                        </kbd>
                    </div>

                    <CommandList className="yzs max-h-[398px] p-[8px]">
                        <CommandEmpty className="py-6 text-center text-[13px] text-(--ink-3)">
                            {t("noSymbols")}
                        </CommandEmpty>
                        {mode === "document" ? (
                            <CommandGroup>
                                {filteredDoc.map((symbol, i) => (
                                    <CommandItem
                                        key={`${symbol.name}-${i}`}
                                        value={String(i)}
                                        onSelect={() => selectDocument(symbol)}
                                        className="h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"
                                    >
                                        <span className="text-[14px] font-medium">{symbol.name}</span>
                                        {symbol.detail ? (
                                            <span className="truncate text-[12px] text-(--ink-3)">{symbol.detail}</span>
                                        ) : null}
                                    </CommandItem>
                                ))}
                            </CommandGroup>
                        ) : (
                            <CommandGroup>
                                {wsSymbols.map((item, i) => (
                                    <CommandItem
                                        key={`${item.uri}-${i}`}
                                        value={String(i)}
                                        onSelect={() => selectWorkspace(item)}
                                        className="h-[42px] gap-[13px] rounded-[12px]! px-[13px] transition-colors duration-100 data-selected:bg-(--yz-active)"
                                    >
                                        <span className="text-[14px] font-medium">{item.name}</span>
                                        <span className="truncate text-[12px] text-(--ink-3)">
                                            {uriToPath(item.uri).split("/").pop()}
                                        </span>
                                    </CommandItem>
                                ))}
                            </CommandGroup>
                        )}
                    </CommandList>
                </Command>
            </DialogContent>
        </Dialog>
    )
}
