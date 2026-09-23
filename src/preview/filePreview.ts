import { previewResourceClose, previewResourceOpen } from "@/lib/ipc"
import type { PreviewResourceLease, PreviewResourceSource } from "@/lib/previewTypes"
import { isSameOrDescendantPath, relativePathWithin } from "@/lib/paths"
import { joinRemoteHostPath, parseRemoteFilePath, remoteFilePath } from "@/lib/runtimeIdentity"
import { retainRemoteWorkspace, remotePreviewSource } from "@/lib/remoteFiles"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { usePreviewStore } from "@/state/previewStore"
import { useUiStore } from "@/state/uiStore"

export type BrowserTarget = { kind: "url"; url: string } | { kind: "file"; workspacePath: string; path: string }
interface Source { id: string; workspacePath: string; root: string; hostId?: string }
interface Lease { id: string; url: string; source: Source; sourceKey: string; assertCurrent: () => void; release?: () => Promise<void> }
// Source identities outlive capabilities so history can reopen after a workspace
// switch. resolveFilePreviewUrl maps this logical URL to a current lease before
// PreviewPanel sends it to the native resource handler.
const sources = new Map<string, Source>()
const leases = new Map<string, Lease>()
let resourceEpoch = 0
let opening: Promise<unknown> = Promise.resolve()

export function isHtmlFile(path: string): boolean { return /\.html?$/i.test(path) }

function resourceUrl(value: string): URL {
    const url = new URL(value)
    if (url.protocol === "http:" && /^yuzora-preview\.[a-f0-9]{32}$/.test(url.hostname)) {
        return new URL(`yuzora-preview://${url.hostname.slice("yuzora-preview.".length)}${url.pathname}${url.search}${url.hash}`)
    }
    return url
}

export function canonicalFilePreviewUrl(value: string): string {
    const url = resourceUrl(value)
    if (url.protocol !== "yuzora-preview:") return value
    if (url.protocol === "yuzora-preview:") {
        const source = leases.get(url.hostname)?.source
        if (source) url.hostname = source.id
    }
    return url.href
}

export function browserTarget(value: string): BrowserTarget {
    const url = resourceUrl(canonicalFilePreviewUrl(value))
    const source = url.protocol === "yuzora-preview:" ? sources.get(url.hostname) : undefined
    if (!source) return { kind: "url", url: value }
    let relative: string
    try { relative = decodeURIComponent(url.pathname.slice(1)) } catch { return { kind: "url", url: value } }
    const path = source.hostId ? joinRemoteHostPath(source.root, relative) : `${source.root.replace(/\/$/, "")}/${relative}`
    return { kind: "file", workspacePath: source.workspacePath, path: source.hostId ? remoteFilePath(source.hostId, path, source.root) : path }
}

async function closeLease(lease: Lease): Promise<void> {
    leases.delete(lease.id)
    try { await previewResourceClose(lease.id) } finally { await lease.release?.() }
}

export async function closeFilePreviews(): Promise<void> {
    resourceEpoch++
    await Promise.allSettled([...leases.values()].map(closeLease))
}

async function acquireFilePreview(workspacePath: string, path: string, source?: Source): Promise<Lease> {
    const epoch = resourceEpoch
    const acquire = async () => {
        const state = useWorkspaceStore.getState()
        if (epoch !== resourceEpoch || state.workspacePath !== workspacePath) throw new Error("Preview workspace changed")
        const remote = parseRemoteFilePath(path)
        const root = parseRemoteFilePath(workspacePath)?.path ?? workspacePath.replaceAll("\\", "/")
        const relative = relativePathWithin(workspacePath, path)
        if (!relative) throw new Error("HTML file is outside the workspace")
        const service = remote ? remotePreviewSource(workspacePath) : null
        if (!service && !state.workspaceCapabilityId) throw new Error("Reopen the workspace before previewing files")
        const capability: PreviewResourceSource = service?.source ?? { kind: "local", workspace: state.workspaceCapabilityId! }
        const assertCurrent = () => {
            if (epoch !== resourceEpoch || useWorkspaceStore.getState().workspacePath !== workspacePath) throw new Error("Preview workspace changed")
            if (service) service.assertCurrent()
            else if (useWorkspaceStore.getState().workspaceCapabilityId !== state.workspaceCapabilityId) throw new Error("Preview workspace changed")
        }
        const sourceKey = JSON.stringify(capability)
        const existing = [...leases.values()].find(entry => entry.source.workspacePath === workspacePath && entry.sourceKey === sourceKey)
        if (existing) { existing.assertCurrent(); return existing }
        // Expired capability leases are never reused by the next Host generation.
        for (const stale of [...leases.values()]) {
            if (stale.source.workspacePath === workspacePath || leases.size >= 8) await closeLease(stale)
        }
        const release = remote ? retainRemoteWorkspace(workspacePath) : undefined
        let opened: PreviewResourceLease | undefined
        try {
            opened = await previewResourceOpen(capability, relative)
            assertCurrent()
            const identity = source ?? [...sources.values()].find(entry => entry.workspacePath === workspacePath)
                ?? { id: opened.id, workspacePath, root, hostId: remote?.hostId }
            sources.set(identity.id, identity)
            const lease = { ...opened, source: identity, sourceKey, assertCurrent, release }
            leases.set(lease.id, lease)
            return lease
        } catch (error) {
            if (opened) await previewResourceClose(opened.id).catch(() => undefined)
            await release?.()
            throw error
        }
    }
    const result = opening.then(acquire)
    opening = result.catch(() => undefined)
    return result
}

/** Resolve a stable source URL into the current connection's resource lease. */
export async function resolveFilePreviewUrl(workspacePath: string, value: string): Promise<string> {
    const target = browserTarget(value)
    if (target.kind !== "file") return value
    if (target.workspacePath !== workspacePath) throw new Error("Preview workspace changed")
    const source = sources.get(resourceUrl(value).hostname)
    const lease = await acquireFilePreview(workspacePath, target.path, source)
    const url = resourceUrl(value)
    url.hostname = lease.id
    return url.href
}

export function assertFilePreviewCurrent(value: string): void {
    const target = browserTarget(value)
    if (target.kind !== "file") return
    const lease = [...leases.values()].find(entry => entry.source.workspacePath === target.workspacePath)
    if (!lease) throw new Error("Preview resource expired")
    lease.assertCurrent()
}

export async function openHtmlPreview(workspacePath: string, path: string, groupIndex: number): Promise<void> {
    if (!isHtmlFile(path) || !isSameOrDescendantPath(workspacePath, path)) throw new Error("HTML file is outside the workspace")
    if (useWorkspaceStore.getState().workspacePath !== workspacePath) return
    const lease = await acquireFilePreview(workspacePath, path)
    lease.assertCurrent()
    const url = resourceUrl(lease.url)
    // Store stable navigation; the renderer resolves the live lease separately.
    url.hostname = lease.source.id
    url.pathname = relativePathWithin(workspacePath, path)!.split("/").map(encodeURIComponent).join("/")
    usePreviewStore.getState().navigate(workspacePath, url.href)
    useWorkspaceStore.getState().openPreviewTab(groupIndex)
    useUiStore.getState().setMode("files")
}

useWorkspaceStore.subscribe((state, previous) => {
    const open = (groups: typeof state.groups) => groups.some((group) => group.tabs.some((tab) => tab.kind === "preview"))
    if (state.workspacePath !== previous.workspacePath || (open(previous.groups) && !open(state.groups))) void closeFilePreviews()
})
