import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useState } from "react"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { GitBranch, Globe, Plus, RefreshCw, Tag } from "lucide-react"
import { useTranslation } from "react-i18next"

import {
    Popover,
    PopoverContent,
    PopoverTrigger
} from "@/components/ui/popover"
import {
    Command,
    CommandGroup,
    CommandInput,
    CommandItem,
    CommandList
} from "@/components/ui/command"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { gitMutationsBlocked, useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import type { BranchInfo, TagInfo } from "@/lib/types"
import { gitCheckout, gitCheckoutDetached, gitCreateBranch, gitFetch, gitPull, gitPush } from "@/lib/ipc"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { requestTextInputDialog } from "@/state/textInputDialogStore"

interface BranchPopoverProps {
    open: boolean
    onOpenChange: (open: boolean) => void
    trigger?: ReactNode
}

type BranchTab = "local" | "remote" | "tags"

const EMPTY_LOCAL: BranchInfo[] = []
const EMPTY_REMOTE: string[] = []
const EMPTY_TAGS: TagInfo[] = []

function hasDirtyTab(): boolean {
    return useWorkspaceStore
        .getState()
        .groups.some((g) => g.tabs.some((t) => t.dirty))
}

function readyRoot(environment: { status: string; root?: string } | null | undefined): string | null {
    return environment?.status === "ready" && environment.root ? environment.root : null
}

function leafName(fullName: string): string {
    const slash = fullName.indexOf("/")
    return slash > 0 ? fullName.slice(slash + 1) : fullName
}

function remoteRevision(name: string): string {
    return `refs/remotes/${name}`
}

function tagRevision(name: string): string {
    return `refs/tags/${name}`
}

function suggestedReleaseBranch(tagName: string): string {
    return `release/${tagName.replace(/^v/, "")}`
}

function ownersForRemote(local: BranchInfo[], remoteName: string): BranchInfo[] {
    return local.filter((branch) => branch.upstream === remoteName)
}

function prefixOf(fullName: string): string | null {
    const slash = fullName.indexOf("/")
    return slash > 0 ? fullName.slice(0, slash + 1) : null
}

function matchesQuery(name: string, query: string): boolean {
    const needle = query.trim().toLowerCase()
    return needle.length === 0 || name.toLowerCase().includes(needle)
}

function groupByPrefix<T extends { name: string }>(
    items: T[],
    otherLabel: string
): Array<{ heading: string; items: T[] }> {
    const map = new Map<string, T[]>()
    for (const item of items) {
        const heading = prefixOf(item.name) ?? otherLabel
        const bucket = map.get(heading)
        if (bucket) bucket.push(item)
        else map.set(heading, [item])
    }
    return [...map].map(([heading, groupItems]) => ({ heading, items: groupItems }))
}

function HighlightedText({ text, query }: { text: string; query: string }) {
    const needle = query.trim()
    if (!needle) return text
    const index = text.toLowerCase().indexOf(needle.toLowerCase())
    if (index < 0) return text
    return (
        <>
            {text.slice(0, index)}
            <mark className="rounded-[2px] bg-(--yz-active) text-inherit">
                {text.slice(index, index + needle.length)}
            </mark>
            {text.slice(index + needle.length)}
        </>
    )
}

function RefScrollList({
    label,
    empty,
    children
}: {
    label: string
    empty: { title: string; description: string } | null
    children: ReactNode
}) {
    return (
        <ScrollArea className="min-h-0 flex-1">
            <CommandList label={label} className="min-h-0 p-[7px]">
                {empty ? (
                    <div className="grid h-[150px] place-items-center px-[20px] text-center text-[10.5px] text-(--ink-3)">
                        <div>
                            <strong className="mb-[4px] block text-(--ink-2)">{empty.title}</strong>
                            {empty.description}
                        </div>
                    </div>
                ) : children}
            </CommandList>
        </ScrollArea>
    )
}

function InlineNotice({ children }: { children: ReactNode }) {
    return (
        <div
            className="mx-[9px] mb-[7px] rounded-[8px] border border-(--line-1) px-[9px] py-[7px] text-[9px] leading-[1.4]"
            style={{ background: "var(--danger-soft)", color: "var(--status-d)" }}
        >
            {children}
        </div>
    )
}

function RowActionHint({ label }: { label: string }) {
    return (
        <span
            data-row-action=""
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 right-[7px] hidden h-[25px] -translate-y-1/2 items-center rounded-[7px] border border-(--line-1) bg-(--yz-solid) px-[7px] text-[8.5px] font-semibold whitespace-nowrap text-(--ink-1) group-hover/command-item:flex group-focus-within/command-item:flex group-data-selected/command-item:flex"
        >
            {label}
        </span>
    )
}

const ACTION_ITEM_CLASS =
    "relative flex min-h-[36px] min-w-0 items-center gap-[7px] rounded-[9px]! px-[7px] py-[4px] data-selected:bg-(--yz-hover) data-selected:shadow-[inset_0_0_0_1px_var(--line-1)] [&>svg:last-child]:hidden"

const ACTIONABLE_ITEM_CLASS =
    `${ACTION_ITEM_CLASS} hover:pr-[4.5rem] focus-within:pr-[4.5rem] data-selected:pr-[4.5rem]`

export function BranchPopover({ open, onOpenChange, trigger }: BranchPopoverProps) {
    const { t } = useTranslation("git")
    const branches = useGitStore((s) => s.branches)
    const busy = useGitStore((s) => s.busy)
    const snapshotStale = useGitStore((s) => s.snapshotStale)
    const remotePaused = useGitStore((s) => s.remotePaused)
    const runOp = useGitStore((s) => s.runOp)
    const environment = useGitStore((s) => s.environment)
    const repositoryRoot = readyRoot(environment)

    // Hide the preview child webview while this popover is open (z-order gate).
    useOverlayPresence(open)

    const [notice, setNotice] = useState<string | null>(null)
    const [creating, setCreating] = useState(false)
    const [newName, setNewName] = useState("")
    const [query, setQuery] = useState("")
    const [tab, setTab] = useState<BranchTab>("local")
    const [selectedRef, setSelectedRef] = useState("")
    const [openSnapshot, setOpenSnapshot] = useState(open)

    // Reset transient UI when the controlled `open` prop closes, including
    // parent-driven closes that never call onOpenChange (T14).
    if (open !== openSnapshot) {
        setOpenSnapshot(open)
        if (!open) {
            setNotice(null)
            setCreating(false)
            setNewName("")
            setQuery("")
            setTab("local")
            setSelectedRef("")
        }
    }

    const mutationsDisabled = gitMutationsBlocked({ busy, snapshotStale, environment })
    const local = branches?.local ?? EMPTY_LOCAL
    const remote = branches?.remote ?? EMPTY_REMOTE
    const tags = branches?.tags ?? EMPTY_TAGS
    const queryTrimmed = query.trim()
    const searching = queryTrimmed.length > 0

    const remoteOwners = useMemo(() => {
        const owners = new Map<string, BranchInfo[]>()
        for (const branch of local) {
            if (!branch.upstream) continue
            const current = owners.get(branch.upstream)
            if (current) current.push(branch)
            else owners.set(branch.upstream, [branch])
        }
        return owners
    }, [local])

    const localMatches = useMemo(
        () => local.filter((branch) => matchesQuery(branch.name, queryTrimmed)),
        [local, queryTrimmed]
    )
    const remoteMatches = useMemo(
        () => remote.filter((name) => matchesQuery(name, queryTrimmed)).map((name) => ({ name })),
        [remote, queryTrimmed]
    )
    const tagMatches = useMemo(
        () => tags.filter((tag) => matchesQuery(tag.name, queryTrimmed)),
        [tags, queryTrimmed]
    )

    const localGroups = useMemo(
        () => searching
            ? [{ heading: t("branchPopover.searchResults", { ns: "menus" }), items: localMatches }]
            : groupByPrefix(localMatches, t("branchPopover.otherGroup", { ns: "menus" })),
        [localMatches, searching, t]
    )
    const remoteGroups = useMemo(
        () => searching
            ? [{ heading: t("branchPopover.searchResults", { ns: "menus" }), items: remoteMatches }]
            : groupByPrefix(remoteMatches, t("branchPopover.otherGroup", { ns: "menus" })),
        [remoteMatches, searching, t]
    )
    const tagGroups = useMemo(
        () => searching
            ? [{ heading: t("branchPopover.searchResults", { ns: "menus" }), items: tagMatches }]
            : groupByPrefix(tagMatches, t("branchPopover.otherGroup", { ns: "menus" })),
        [tagMatches, searching, t]
    )

    const activeItems = tab === "local" ? localMatches : tab === "remote" ? remoteMatches : tagMatches
    const activeTotal = tab === "local" ? local.length : tab === "remote" ? remote.length : tags.length
    const selectedTag = tagMatches.find((tag) => tag.name === selectedRef) ?? tagMatches[0] ?? null
    const selectedCopyName = tab === "remote"
        ? remoteMatches.find((item) => item.name === selectedRef)?.name ?? remoteMatches[0]?.name
        : tab === "tags" ? selectedTag?.name : undefined

    useEffect(() => {
        if (!activeItems.some((item) => item.name === selectedRef)) {
            setSelectedRef(activeItems[0]?.name ?? "")
        }
    }, [activeItems, selectedRef])

    function dirtyBlocked(messageKey = "branchPopover.dirtyTabsBlockCheckout"): boolean {
        if (!hasDirtyTab()) return false
        setNotice(t(messageKey, { ns: "menus" }))
        return true
    }

    async function checkout(fullName: string) {
        if (mutationsDisabled || dirtyBlocked()) return
        const capturedRoot = repositoryRoot
        if (!capturedRoot) return
        const ok = await runOp("checkout", () => gitCheckout(capturedRoot, fullName))
        if (ok) onOpenChange(false)
    }

    function activateLocal(branch: BranchInfo) {
        if (branch.isCurrent || mutationsDisabled) return
        void checkout(branch.name)
    }

    async function activateRemote(fullName: string) {
        if (mutationsDisabled || dirtyBlocked("branchPopover.dirtyTabsBlockRemote")) return
        const capturedRoot = repositoryRoot
        if (!capturedRoot) return
        const owners = ownersForRemote(local, fullName)
        if (owners.length > 1) {
            setNotice(t("branchPopover.multipleRemoteOwners", { ns: "menus", name: fullName }))
            return
        }
        if (owners.length === 1) {
            if (owners[0].isCurrent) return
            const ok = await runOp("checkout", () => gitCheckout(capturedRoot, owners[0].name))
            if (ok) onOpenChange(false)
            return
        }

        const derived = leafName(fullName)
        let localName = derived
        if (local.some((branch) => branch.name === derived)) {
            const name = await requestTextInputDialog({
                title: t("branchPopover.alternateLocalTitle", { ns: "menus" }),
                description: t("branchPopover.alternateLocalDescription", {
                    ns: "menus",
                    derived,
                    remote: fullName
                }),
                label: t("branchPopover.branchNameLabel", { ns: "menus" }),
                initialValue: derived,
                placeholder: t("branchNamePlaceholder"),
                confirmLabel: t("branchPopover.createBranch", { ns: "menus" })
            })
            const trimmed = name?.trim()
            if (!trimmed) return
            localName = trimmed
        }

        const live = useGitStore.getState()
        const liveRoot = readyRoot(live.environment)
        if (liveRoot !== capturedRoot || gitMutationsBlocked(live) || dirtyBlocked("branchPopover.dirtyTabsBlockRemote")) return
        const liveLocal = live.branches?.local ?? EMPTY_LOCAL
        const liveOwners = ownersForRemote(liveLocal, fullName)
        if (liveOwners.length > 1) {
            setNotice(t("branchPopover.multipleRemoteOwners", { ns: "menus", name: fullName }))
            return
        }
        if (liveOwners.length === 1) {
            if (liveOwners[0].isCurrent) return
            const ok = await runOp("checkout", () => gitCheckout(capturedRoot, liveOwners[0].name))
            if (ok) onOpenChange(false)
            return
        }
        if (liveLocal.some((branch) => branch.name === localName)) {
            setNotice(t("branchPopover.localNameTaken", { ns: "menus", name: localName }))
            return
        }
        const ok = await runOp("create-branch", () => gitCreateBranch(capturedRoot, localName, remoteRevision(fullName)))
        if (ok) onOpenChange(false)
    }

    async function createBranchFromTag(tag: TagInfo) {
        if (mutationsDisabled || dirtyBlocked("branchPopover.dirtyTabsBlockCreate")) return
        const capturedRoot = repositoryRoot
        if (!capturedRoot) return
        const name = await requestTextInputDialog({
            title: t("branchPopover.createFromTagTitle", { ns: "menus" }),
            description: t("branchPopover.createFromTagDescription", { ns: "menus", name: tag.name }),
            label: t("branchPopover.branchNameLabel", { ns: "menus" }),
            initialValue: suggestedReleaseBranch(tag.name),
            placeholder: t("branchNamePlaceholder"),
            confirmLabel: t("branchPopover.createBranch", { ns: "menus" })
        })
        const trimmed = name?.trim()
        if (!trimmed) return
        const liveRoot = readyRoot(useGitStore.getState().environment)
        if (liveRoot !== capturedRoot || gitMutationsBlocked() || dirtyBlocked("branchPopover.dirtyTabsBlockCreate")) return
        const ok = await runOp("create-branch", () => gitCreateBranch(capturedRoot, trimmed, tagRevision(tag.name)))
        if (ok) onOpenChange(false)
    }

    async function checkoutTag(tag: TagInfo) {
        if (mutationsDisabled || dirtyBlocked("branchPopover.dirtyTabsBlockTag")) return
        const capturedRoot = repositoryRoot
        if (!capturedRoot) return
        const accepted = await requestAppConfirmation({
            title: t("branchPopover.checkoutTagTitle", { ns: "menus" }),
            description: t("branchPopover.checkoutTagDescription", { ns: "menus", name: tag.name }),
            confirmLabel: t("branchPopover.checkoutDetached", { ns: "menus" }),
            cancelLabel: t("branchPopover.cancel", { ns: "menus" }),
            kind: "warning"
        })
        if (!accepted) {
            const liveRoot = readyRoot(useGitStore.getState().environment)
            if (liveRoot !== capturedRoot || gitMutationsBlocked() || dirtyBlocked("branchPopover.dirtyTabsBlockCreate")) return
            await createBranchFromTag(tag)
            return
        }
        const liveRoot = readyRoot(useGitStore.getState().environment)
        if (liveRoot !== capturedRoot || gitMutationsBlocked() || dirtyBlocked("branchPopover.dirtyTabsBlockTag")) return
        const ok = await runOp("checkout", () => gitCheckoutDetached(capturedRoot, tagRevision(tag.name)))
        if (ok) onOpenChange(false)
    }

    async function copyRef(fullName: string) {
        try {
            await writeText(fullName)
        } catch {
            setNotice(t("branchPopover.copyFailed", { ns: "menus" }))
        }
    }

    async function createBranch() {
        const name = newName.trim()
        if (!name || mutationsDisabled || dirtyBlocked("branchPopover.dirtyTabsBlockCreate")) return
        const capturedRoot = repositoryRoot
        if (!capturedRoot) return
        const ok = await runOp("create-branch", () => gitCreateBranch(capturedRoot, name))
        if (ok) {
            setNewName("")
            setCreating(false)
        }
    }

    function runBoundOp(name: "fetch" | "pull" | "push", fn: (root: string) => Promise<unknown>) {
        const capturedRoot = repositoryRoot
        if (!capturedRoot || mutationsDisabled) return
        void runOp(name, () => fn(capturedRoot))
    }

    function handleSearchKeyDown(event: KeyboardEvent<HTMLInputElement>) {
        if (event.key === "Escape" && query.length > 0) {
            event.preventDefault()
            event.stopPropagation()
            setQuery("")
        }
    }

    function emptyCopy() {
        if (searching) {
            return {
                title: t("branchPopover.emptySearchTitle", { ns: "menus" }),
                description: t("branchPopover.emptySearchDescription", { ns: "menus" })
            }
        }
        if (tab === "local") {
            return {
                title: t("branchPopover.emptyLocalTitle", { ns: "menus" }),
                description: t("branchPopover.emptyLocalDescription", { ns: "menus" })
            }
        }
        if (tab === "remote") {
            return {
                title: t("branchPopover.emptyRemoteTitle", { ns: "menus" }),
                description: t("branchPopover.emptyRemoteDescription", { ns: "menus" })
            }
        }
        return {
            title: t("branchPopover.emptyTagsTitle", { ns: "menus" }),
            description: t("branchPopover.emptyTagsDescription", { ns: "menus" })
        }
    }

    const empty = emptyCopy()

    return (
        <Popover open={open} onOpenChange={onOpenChange}>
            {trigger ? <PopoverTrigger asChild>{trigger}</PopoverTrigger> : <PopoverTrigger />}
            <PopoverContent
                side="bottom"
                align="start"
                sideOffset={6}
                collisionPadding={12}
                onOpenAutoFocus={(event) => {
                    event.preventDefault()
                    const root = event.currentTarget
                    if (root instanceof HTMLElement) {
                        root.querySelector<HTMLInputElement>("[data-slot=command-input]")?.focus()
                    }
                }}
                onEscapeKeyDown={(event) => {
                    if (query.length > 0) {
                        event.preventDefault()
                        setQuery("")
                    }
                }}
                className="yz-pop flex h-[min(68vh,520px)] w-[340px] max-w-[92vw] min-w-0 flex-col gap-0 overflow-hidden rounded-[14px] border border-(--line-2) bg-(--frost-light) p-0 shadow-[var(--shadow-xl)] ring-0 backdrop-blur-[20px]"
            >
                <div className="flex shrink-0 items-center gap-[8px] px-[12px] pt-[11px] pb-[9px]">
                    <GitBranch
                        className="size-[15px] shrink-0"
                        style={{ color: "var(--yz-accent-ink)" }}
                        aria-hidden="true"
                    />
                    <span className="min-w-0 flex-1 font-serif text-[15px] font-semibold text-(--ink-0)">
                        {t("branchPopover.title", { ns: "menus" })}
                    </span>
                    <Button
                        type="button"
                        variant="outline"
                        size="xs"
                        aria-label={t("branchPopover.fetchRemoteAriaLabel", { ns: "menus" })}
                        disabled={mutationsDisabled}
                        onClick={() => runBoundOp("fetch", (root) => gitFetch(root, false))}
                        className="flex h-[26px] items-center gap-[5px] rounded-[8px] border border-(--line-1) bg-(--yz-solid) px-[9px] text-[10px] font-semibold text-(--ink-1) shadow-[var(--shadow-xs)] transition-colors hover:bg-(--paper-1) disabled:opacity-50"
                    >
                        <RefreshCw className="size-[12px]" aria-hidden="true" />
                        {busy === "fetch"
                            ? t("branchPopover.fetchingEllipsis", { ns: "menus" })
                            : t("branchPopover.fetch", { ns: "menus" })}
                    </Button>
                </div>

                <Command
                    shouldFilter={false}
                    value={selectedRef}
                    onValueChange={setSelectedRef}
                    className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-none! bg-transparent p-0"
                    onKeyDown={(event) => {
                        if (event.key === "Escape" && query.length > 0) {
                            event.preventDefault()
                            event.stopPropagation()
                            setQuery("")
                        }
                    }}
                >
                    <CommandInput
                        autoFocus
                        autoComplete="off"
                        value={query}
                        onValueChange={setQuery}
                        onKeyDown={handleSearchKeyDown}
                        placeholder={t("branchPopover.searchPlaceholder", { ns: "menus" })}
                        aria-label={t("branchPopover.searchAriaLabel", { ns: "menus" })}
                    />

                    <Tabs
                        value={tab}
                        onValueChange={(value) => {
                            if (value !== "local" && value !== "remote" && value !== "tags") return
                            setTab(value)
                            setQuery("")
                            setNotice(null)
                        }}
                        className="flex min-h-0 flex-1 flex-col gap-0"
                    >
                        <TabsList className="mx-[9px] mt-[8px] mb-[6px] grid h-[29px] w-[calc(100%-18px)] grid-cols-3">
                            <TabsTrigger
                                value="local"
                                aria-label={t("branchPopover.tabCount", { ns: "menus", tab: t("branchPopover.localSection", { ns: "menus" }), count: local.length })}
                                className="text-[10px] font-semibold"
                            >
                                <span>{t("branchPopover.localSection", { ns: "menus" })}</span>
                                <span className="font-mono text-[8.5px] text-(--ink-4)">{local.length}</span>
                            </TabsTrigger>
                            <TabsTrigger
                                value="remote"
                                aria-label={t("branchPopover.tabCount", { ns: "menus", tab: t("branchPopover.remoteSection", { ns: "menus" }), count: remote.length })}
                                className="text-[10px] font-semibold"
                            >
                                <span>{t("branchPopover.remoteSection", { ns: "menus" })}</span>
                                <span className="font-mono text-[8.5px] text-(--ink-4)">{remote.length}</span>
                            </TabsTrigger>
                            <TabsTrigger
                                value="tags"
                                aria-label={t("branchPopover.tabCount", { ns: "menus", tab: t("branchPopover.tagsSection", { ns: "menus" }), count: tags.length })}
                                className="text-[10px] font-semibold"
                            >
                                <span>{t("branchPopover.tagsSection", { ns: "menus" })}</span>
                                <span className="font-mono text-[8.5px] text-(--ink-4)">{tags.length}</span>
                            </TabsTrigger>
                        </TabsList>

                    <div className="flex shrink-0 items-center justify-between gap-[8px] px-[13px] pb-[5px] text-[8.5px] text-(--ink-4)">
                        <span>
                            {tab === "local"
                                ? t("branchPopover.resultsLocal", { ns: "menus" })
                                : tab === "remote"
                                    ? t("branchPopover.resultsRemote", { ns: "menus" })
                                    : t("branchPopover.resultsTags", { ns: "menus" })}
                        </span>
                        <span className="flex items-center gap-[6px]">
                            <span>
                                {searching
                                    ? t("branchPopover.resultCountFiltered", {
                                        ns: "menus",
                                        shown: activeItems.length,
                                        total: activeTotal
                                    })
                                    : t("branchPopover.resultCount", { ns: "menus", count: activeTotal })}
                            </span>
                            {selectedCopyName && (
                                <Button
                                    type="button"
                                    variant="ghost"
                                    size="xs"
                                    aria-label={t("branchPopover.copyAriaLabel", { ns: "menus", name: selectedCopyName })}
                                    onClick={() => void copyRef(selectedCopyName)}
                                    className="h-[22px] px-[6px] text-[8.5px]"
                                >
                                    {t("branchPopover.copy", { ns: "menus" })}
                                </Button>
                            )}
                        </span>
                    </div>

                    {remotePaused && (
                        <InlineNotice>{t("branchPopover.remoteCheckPaused", { ns: "menus" })}</InlineNotice>
                    )}
                    {mutationsDisabled && (
                        <InlineNotice>{t("branchPopover.browseOnly", { ns: "menus" })}</InlineNotice>
                    )}
                    {notice && <InlineNotice>{notice}</InlineNotice>}

                    <TabsContent value="local" className="flex min-h-0 flex-1 flex-col overflow-hidden outline-none">
                        <RefScrollList
                            label={t("branchPopover.resultsLocal", { ns: "menus" })}
                            empty={localMatches.length === 0 ? empty : null}
                        >
                            {localGroups.map((group) => (
                                <CommandGroup
                                    key={group.heading}
                                    heading={
                                        <span className="flex items-center gap-[5px]">
                                            <span>{group.heading}</span>
                                            <span className="font-mono font-normal text-(--ink-4)">{group.items.length}</span>
                                        </span>
                                    }
                                >
                                    {group.items.map((branch) => (
                                        <LocalBranchItem
                                            key={branch.name}
                                            branch={branch}
                                            query={queryTrimmed}
                                            searching={searching}
                                            onActivate={() => activateLocal(branch)}
                                        />
                                    ))}
                                </CommandGroup>
                            ))}
                        </RefScrollList>
                    </TabsContent>
                    <TabsContent value="remote" className="flex min-h-0 flex-1 flex-col overflow-hidden outline-none">
                        <RefScrollList
                            label={t("branchPopover.resultsRemote", { ns: "menus" })}
                            empty={remoteMatches.length === 0 ? empty : null}
                        >
                            {remoteGroups.map((group) => (
                                <CommandGroup
                                    key={group.heading}
                                    heading={
                                        <span className="flex items-center gap-[5px]">
                                            <span>{group.heading}</span>
                                            <span className="font-mono font-normal text-(--ink-4)">{group.items.length}</span>
                                        </span>
                                    }
                                >
                                    {group.items.map((item) => (
                                        <RemoteBranchItem
                                            key={item.name}
                                            name={item.name}
                                            query={queryTrimmed}
                                            searching={searching}
                                            tracked={(remoteOwners.get(item.name)?.length ?? 0) > 0}
                                            onActivate={() => void activateRemote(item.name)}
                                        />
                                    ))}
                                </CommandGroup>
                            ))}
                        </RefScrollList>
                    </TabsContent>
                    <TabsContent value="tags" className="flex min-h-0 flex-1 flex-col overflow-hidden outline-none">
                        <RefScrollList
                            label={t("branchPopover.resultsTags", { ns: "menus" })}
                            empty={tagMatches.length === 0 ? empty : null}
                        >
                            {tagGroups.map((group) => (
                                <CommandGroup
                                    key={group.heading}
                                    heading={
                                        <span className="flex items-center gap-[5px]">
                                            <span>{group.heading}</span>
                                            <span className="font-mono font-normal text-(--ink-4)">{group.items.length}</span>
                                        </span>
                                    }
                                >
                                    {group.items.map((tag) => (
                                        <TagItem
                                            key={tag.name}
                                            tag={tag}
                                            query={queryTrimmed}
                                            searching={searching}
                                            onActivate={() => void checkoutTag(tag)}
                                        />
                                    ))}
                                </CommandGroup>
                            ))}
                        </RefScrollList>
                    </TabsContent>
                    </Tabs>
                </Command>

                <div className="shrink-0 border-t border-(--line-1) p-[7px]">
                    {tab === "tags" && selectedTag && (
                        <div className="mb-[6px] grid grid-cols-2 gap-[5px]">
                            <Button
                                type="button"
                                variant="outline"
                                size="xs"
                                disabled={mutationsDisabled}
                                onClick={() => void checkoutTag(selectedTag)}
                                className="h-[28px] truncate px-[7px] text-[9px]"
                            >
                                {t("branchPopover.checkoutDetached", { ns: "menus" })}
                            </Button>
                            <Button
                                type="button"
                                variant="outline"
                                size="xs"
                                disabled={mutationsDisabled}
                                onClick={() => void createBranchFromTag(selectedTag)}
                                aria-label={t("branchPopover.createFromTagAriaLabel", { ns: "menus", name: selectedTag.name })}
                                className="h-[28px] truncate px-[7px] text-[9px]"
                            >
                                {t("branchPopover.createFromTag", { ns: "menus" })}
                            </Button>
                        </div>
                    )}
                    {creating ? (
                        <Input
                            autoFocus
                            disabled={mutationsDisabled}
                            value={newName}
                            placeholder={t("branchNamePlaceholder")}
                            onChange={(e) => setNewName(e.target.value)}
                            onKeyDown={(e) => {
                                if (e.key === "Enter") void createBranch()
                                if (e.key === "Escape") {
                                    e.preventDefault()
                                    e.stopPropagation()
                                    setNewName("")
                                    setCreating(false)
                                }
                            }}
                            onBlur={() => {
                                if (!newName.trim()) setCreating(false)
                            }}
                            className="h-[30px] w-full rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[11px] font-mono text-[12px] text-(--ink-1) outline-none"
                        />
                    ) : (
                        <Button
                            type="button"
                            variant="ghost"
                            disabled={mutationsDisabled}
                            onClick={() => setCreating(true)}
                            className="flex h-[30px] w-full items-center gap-[9px] rounded-[9px] px-[11px] text-[12px] font-medium text-(--ink-2) transition-colors duration-100 hover:bg-(--yz-hover)"
                        >
                            <Plus className="size-[13px] shrink-0" aria-hidden="true" />
                            {t("branchPopover.newBranch", { ns: "menus" })}
                        </Button>
                    )}
                </div>

                <div className="grid shrink-0 grid-cols-3 items-center gap-[5px] border-t border-(--line-1) px-[9px] py-[7px]">
                    <ActionButton
                        opKey="fetch"
                        label={t("branchPopover.fetch", { ns: "menus" })}
                        busy={busy}
                        disabled={mutationsDisabled}
                        onClick={() => runBoundOp("fetch", (root) => gitFetch(root, false))}
                    />
                    <ActionButton
                        opKey="pull"
                        label={t("branchPopover.pull", { ns: "menus" })}
                        busy={busy}
                        disabled={mutationsDisabled}
                        onClick={() => runBoundOp("pull", (root) => gitPull(root))}
                    />
                    <ActionButton
                        opKey="push"
                        label={t("branchPopover.push", { ns: "menus" })}
                        busy={busy}
                        disabled={mutationsDisabled}
                        onClick={() => runBoundOp("push", (root) => gitPush(root))}
                    />
                </div>
            </PopoverContent>
        </Popover>
    )
}

function LocalBranchItem({
    branch,
    query,
    searching,
    onActivate
}: {
    branch: BranchInfo
    query: string
    searching: boolean
    onActivate: () => void
}) {
    const { t } = useTranslation("git")
    const primary = searching ? branch.name : leafName(branch.name)
    const secondary = branch.upstream ?? t("branchPopover.noUpstream", { ns: "menus" })
    const diverged = !branch.gone && branch.ahead > 0 && branch.behind > 0

    return (
        <CommandItem
            value={branch.name}
            title={branch.name}
            aria-label={branch.name}
            onSelect={onActivate}
            data-has-action={branch.isCurrent ? undefined : "true"}
            className={branch.isCurrent ? ACTION_ITEM_CLASS : ACTIONABLE_ITEM_CLASS}
        >
            <GitBranch className="size-[11px] shrink-0 text-(--ink-4)" aria-hidden="true" />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex min-w-0 items-center gap-[5px]">
                    <span className="min-w-0 truncate font-mono text-[10.5px] font-semibold text-(--ink-1)">
                        <HighlightedText text={primary} query={query} />
                    </span>
                    {branch.isCurrent && (
                        <Badge className="h-[16px] border-transparent bg-(--yz-active) px-[5px] text-[7.8px] font-bold text-(--yz-accent-ink)">
                            {t("branchPopover.current", { ns: "menus" })}
                        </Badge>
                    )}
                    {branch.gone && (
                        <Badge className="h-[16px] border-transparent bg-[#fbf0db] px-[5px] text-[7.8px] font-bold text-[#9a6512]">
                            {t("branchPopover.gone", { ns: "menus" })}
                        </Badge>
                    )}
                    {!branch.isCurrent && !branch.gone && branch.upstream == null && (
                        <Badge variant="secondary" className="h-[16px] px-[5px] text-[7.8px] font-bold text-(--ink-3)">
                            {t("branchPopover.localOnly", { ns: "menus" })}
                        </Badge>
                    )}
                    {diverged && (
                        <Badge variant="outline" className="h-[16px] px-[5px] text-[7.8px] font-bold">
                            {t("branchPopover.diverged", { ns: "menus" })}
                        </Badge>
                    )}
                </span>
                <span className="mt-[3px] min-w-0 truncate font-mono text-[8.5px] text-(--ink-4)">
                    {secondary}
                </span>
            </span>
            {!branch.gone && (branch.ahead > 0 || branch.behind > 0) && (
                <span className="flex shrink-0 gap-[3px] font-mono text-[8.5px] font-semibold">
                    {branch.ahead > 0 && (
                        <span style={{ color: "var(--status-m)" }}>↑{branch.ahead}</span>
                    )}
                    {branch.behind > 0 && (
                        <span style={{ color: "#c8521f" }}>↓{branch.behind}</span>
                    )}
                </span>
            )}
            {!branch.isCurrent && (
                <RowActionHint label={t("branchPopover.checkout", { ns: "menus" })} />
            )}
        </CommandItem>
    )
}

function RemoteBranchItem({
    name,
    query,
    searching,
    tracked,
    onActivate
}: {
    name: string
    query: string
    searching: boolean
    tracked: boolean
    onActivate: () => void
}) {
    const { t } = useTranslation("git")
    const primary = searching ? name : leafName(name)
    const secondary = tracked
        ? t("branchPopover.trackedByLocal", { ns: "menus" })
        : t("branchPopover.remoteBranch", { ns: "menus" })

    return (
        <CommandItem
            value={name}
            title={name}
            aria-label={name}
            onSelect={onActivate}
            data-has-action="true"
            className={ACTIONABLE_ITEM_CLASS}
        >
            <Globe className="size-[11px] shrink-0 text-(--ink-4)" aria-hidden="true" />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="flex min-w-0 items-center gap-[5px]">
                    <span className="min-w-0 truncate font-mono text-[10.5px] font-semibold text-(--ink-1)">
                        <HighlightedText text={primary} query={query} />
                    </span>
                    {tracked && (
                        <Badge className="h-[16px] border-transparent bg-[#e8f5ef] px-[5px] text-[7.8px] font-bold text-[#178a63]">
                            {t("branchPopover.tracked", { ns: "menus" })}
                        </Badge>
                    )}
                </span>
                <span className="mt-[3px] min-w-0 truncate font-mono text-[8.5px] text-(--ink-4)">
                    {secondary}
                </span>
            </span>
            <RowActionHint label={t("branchPopover.checkoutAsLocal", { ns: "menus" })} />
        </CommandItem>
    )
}

function TagItem({
    tag,
    query,
    searching,
    onActivate
}: {
    tag: TagInfo
    query: string
    searching: boolean
    onActivate: () => void
}) {
    const { t } = useTranslation("git")
    const primary = searching ? tag.name : leafName(tag.name)
    return (
        <CommandItem
            value={tag.name}
            title={tag.name}
            aria-label={tag.name}
            onSelect={onActivate}
            data-has-action="true"
            className={ACTIONABLE_ITEM_CLASS}
        >
            <Tag className="size-[11px] shrink-0 text-[#2456cc]" aria-hidden="true" />
            <span className="flex min-w-0 flex-1 flex-col">
                <span className="min-w-0 truncate font-mono text-[10.5px] font-semibold text-(--ink-1)">
                    <HighlightedText text={primary} query={query} />
                </span>
                <span className="mt-[3px] min-w-0 truncate font-mono text-[8.5px] text-(--ink-4)">
                    {tag.date}
                </span>
            </span>
            <RowActionHint label={t("branchPopover.checkoutDetached", { ns: "menus" })} />
        </CommandItem>
    )
}

function ActionButton({
    opKey,
    label,
    busy,
    disabled = false,
    onClick
}: {
    opKey: string
    label: string
    busy: string | null
    disabled?: boolean
    onClick: () => void
}) {
    // Compared against the untranslated op id (not the display label) so a
    // localized label doesn't break the busy/spinner match (label is now
    // translated; busy stays the internal "fetch"/"pull"/"push" identifier).
    const active = busy === opKey
    return (
        <Button
            type="button"
            variant="outline"
            disabled={disabled || busy != null}
            onClick={onClick}
            className="flex h-[29px] flex-1 items-center justify-center gap-[6px] rounded-[8px] border border-(--line-1) bg-(--yz-solid) text-[9.5px] font-semibold text-(--ink-1) shadow-[var(--shadow-xs)] transition-colors hover:bg-(--paper-1) disabled:opacity-50"
        >
            {active ? `${label}…` : label}
        </Button>
    )
}
