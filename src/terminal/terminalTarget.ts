import type { Terminal } from "@xterm/xterm"

import { getDocument } from "../editor/documentRegistry"
import { showActionError } from "../lib/actionFeedback"
import { isOpenableFile } from "../lib/ipc"
import i18n from "../lib/i18n"
import { isAbsolutePath, nativePathJoin } from "../lib/paths"
import { isMacPlatform } from "../lib/platform"
import type { HerdrSnapshot } from "../lib/herdrTypes"
import { usePreviewStore } from "../state/previewStore"
import { useWorkspaceStore } from "../state/workspaceStore"

export type TerminalFileTarget = {
    kind: "file"
    path: string
    line?: number
    column?: number
}

export type TerminalUrlTarget = {
    kind: "url"
    url: string
}

export type TerminalTarget = TerminalFileTarget | TerminalUrlTarget

export interface TerminalTargetMatch {
    target: TerminalTarget
    start: number
    end: number
    text: string
}

export interface TerminalTargetBufferCell {
    getChars(): string
    getWidth(): number
}

export interface TerminalTargetBufferLine {
    readonly isWrapped: boolean
    readonly length: number
    getCell(x: number): TerminalTargetBufferCell | undefined
    translateToString(trimRight?: boolean, startColumn?: number, endColumn?: number): string
}

export interface TerminalTargetBuffer {
    readonly length: number
    getLine(y: number): TerminalTargetBufferLine | undefined
}

export interface TerminalTargetBufferHost {
    readonly cols: number
    readonly buffer: { readonly active: TerminalTargetBuffer }
}

export interface TerminalTargetLinkRange {
    start: { x: number; y: number }
    end: { x: number; y: number }
}

export interface TerminalTargetLink {
    range: TerminalTargetLinkRange
    text: string
    decorations?: { pointerCursor: boolean; underline: boolean }
    activate: (event: MouseEvent, text: string) => void
    hover?: (event: MouseEvent, text: string) => void
    leave?: (event: MouseEvent, text: string) => void
}

/* eslint-disable-next-line no-control-regex -- reject C0/DEL so terminal tokens cannot smuggle control chars */
const CONTROL_CHARS = /[\u0000-\u001F\u007F]/
const ALLOWED_URL_PROTOCOLS = new Set(["http:", "https:"])
const RELATIVE_FILE_EXT = /\.[A-Za-z0-9]{1,10}$/

export function hasDisallowedControlChars(value: string): boolean {
    return CONTROL_CHARS.test(value)
}

export function isTerminalTargetOpenGesture(
    event: Pick<MouseEvent, "button" | "metaKey" | "ctrlKey" | "altKey">,
    platform: { mac?: boolean } = {}
): boolean {
    if (event.button !== 0 || event.altKey === true) return false
    const mac = platform.mac ?? isMacPlatform()
    if (mac) return event.metaKey === true && event.ctrlKey !== true
    return event.ctrlKey === true && event.metaKey !== true
}

export function isTerminalTargetModifierHeld(
    event: Pick<MouseEvent, "metaKey" | "ctrlKey" | "altKey">,
    platform: { mac?: boolean } = {}
): boolean {
    if (event.altKey === true) return false
    const mac = platform.mac ?? isMacPlatform()
    if (mac) return event.metaKey === true && event.ctrlKey !== true
    return event.ctrlKey === true && event.metaKey !== true
}

function targetKey(target: TerminalTarget): string {
    if (target.kind === "url") return `url:${target.url}`
    return `file:${target.path}:${target.line ?? ""}:${target.column ?? ""}`
}

function trimTrailingPunctuation(raw: string): string {
    let value = raw
    while (value.length > 0) {
        const last = value[value.length - 1]
        if (/[.,;:!?']/.test(last)) {
            value = value.slice(0, -1)
            continue
        }
        if (last === ")" && (value.match(/\(/g)?.length ?? 0) < (value.match(/\)/g)?.length ?? 0)) {
            value = value.slice(0, -1)
            continue
        }
        if (last === "]" && (value.match(/\[/g)?.length ?? 0) < (value.match(/\]/g)?.length ?? 0)) {
            value = value.slice(0, -1)
            continue
        }
        if (last === ">" && (value.match(/</g)?.length ?? 0) < (value.match(/>/g)?.length ?? 0)) {
            value = value.slice(0, -1)
            continue
        }
        break
    }
    return value
}

function unwrapToken(raw: string): { text: string; lead: number } {
    let start = 0
    let end = raw.length
    const pairs: Record<string, string> = { "(": ")", "[": "]", "{": "}", "<": ">" }
    while (end > start + 1 && pairs[raw[start]] && raw[end - 1] === pairs[raw[start]]) {
        start += 1
        end -= 1
    }
    return { text: raw.slice(start, end), lead: start }
}

function schemeName(raw: string): string | null {
    const match = raw.match(/^([A-Za-z][A-Za-z0-9+.-]*):/)
    return match ? match[1].toLowerCase() : null
}

function isWindowsDrivePath(raw: string): boolean {
    return /^[A-Za-z]:[\\/]/.test(raw)
}

function looksLikeRelativeFile(path: string): boolean {
    if (path.startsWith("./") || path.startsWith(".\\") || path.startsWith("../") || path.startsWith("..\\")) {
        return true
    }
    if (path.includes("/") || path.includes("\\")) return true
    return RELATIVE_FILE_EXT.test(path)
}

function splitLineColumn(raw: string): { path: string; line?: number; column?: number } {
    const match = raw.match(/^(.*?):(\d+)(?::(\d+))?$/)
    if (!match) return { path: raw }
    const path = match[1]
    if (/^[A-Za-z]:$/.test(path)) return { path: raw }
    const line = Number(match[2])
    if (!Number.isInteger(line) || line < 1) return { path: raw }
    const column = match[3] === undefined ? undefined : Number(match[3])
    if (column !== undefined && (!Number.isInteger(column) || column < 1)) {
        return { path, line }
    }
    return { path, line, column }
}

function fileTarget(path: string, line?: number, column?: number): TerminalFileTarget {
    const target: TerminalFileTarget = { kind: "file", path }
    if (line !== undefined) target.line = line
    if (column !== undefined) target.column = column
    return target
}

function stripLeadingDotSegments(path: string): string {
    return path.replace(/^(?:\.[\\/])+/, "")
}

function decodeStrict(value: string): string | null {
    try {
        return decodeURIComponent(value)
    } catch {
        return null
    }
}

function parseHttpUrl(raw: string): TerminalUrlTarget | null {
    const trimmed = trimTrailingPunctuation(raw)
    if (!trimmed || hasDisallowedControlChars(trimmed)) return null
    try {
        const url = new URL(trimmed)
        if (!ALLOWED_URL_PROTOCOLS.has(url.protocol)) return null
        if (hasDisallowedControlChars(url.href)) return null
        return { kind: "url", url: url.href }
    } catch {
        return null
    }
}

function parseFileUrl(raw: string): TerminalFileTarget | null {
    const trimmed = trimTrailingPunctuation(raw)
    if (!trimmed || hasDisallowedControlChars(trimmed)) return null
    let parsed: URL
    try {
        parsed = new URL(trimmed)
    } catch {
        return null
    }
    if (parsed.protocol !== "file:") return null
    const decodedPath = decodeStrict(parsed.pathname)
    if (decodedPath === null) return null
    if (hasDisallowedControlChars(decodedPath) || hasDisallowedControlChars(parsed.href)) return null
    const { path, line, column } = splitLineColumn(decodedPath)

    let localPath = path
    const host = parsed.hostname
    if (host && host !== "localhost" && host !== "127.0.0.1") {
        localPath = `\\\\${host}${path.replace(/\//g, "\\")}`
    } else if (/^\/[A-Za-z]:[\\/]/.test(path)) {
        localPath = path.slice(1)
    }

    if (!isAbsolutePath(localPath)) return null
    return fileTarget(localPath, line, column)
}

function parseFilePath(raw: string, cwd: string | null): TerminalFileTarget | null {
    const trimmed = trimTrailingPunctuation(raw)
    if (!trimmed || hasDisallowedControlChars(trimmed)) return null
    const { path, line, column } = splitLineColumn(trimmed)
    if (!path || hasDisallowedControlChars(path)) return null

    if (isAbsolutePath(path)) {
        return fileTarget(path, line, column)
    }
    if (!cwd || !isAbsolutePath(cwd) || !looksLikeRelativeFile(path)) return null
    return fileTarget(nativePathJoin(cwd, stripLeadingDotSegments(path)), line, column)
}

export function classifyTerminalTargetToken(
    raw: string,
    cwd: string | null
): TerminalTarget | null {
    if (!raw || hasDisallowedControlChars(raw)) return null
    const scheme = schemeName(raw)
    if (scheme) {
        if (scheme === "http" || scheme === "https") return parseHttpUrl(raw)
        if (scheme === "file") return parseFileUrl(raw)
        if (isWindowsDrivePath(raw)) return parseFilePath(raw, cwd)
        return null
    }
    return parseFilePath(raw, cwd)
}

function pushMatch(
    matches: TerminalTargetMatch[],
    raw: string,
    start: number,
    cwd: string | null
): void {
    const unwrapped = unwrapToken(raw)
    const candidate = unwrapped.text
    const target = classifyTerminalTargetToken(candidate, cwd)
    if (!target) return
    const consumed = trimTrailingPunctuation(candidate)
    if (!consumed) return
    const matchStart = start + unwrapped.lead
    matches.push({
        target,
        start: matchStart,
        end: matchStart + consumed.length,
        text: consumed
    })
}

export function findTerminalTargetsInText(
    text: string,
    cwd: string | null
): TerminalTargetMatch[] {
    if (!text || hasDisallowedControlChars(text)) return []
    const matches: TerminalTargetMatch[] = []
    let index = 0
    while (index < text.length) {
        const char = text[index]
        if (/\s/.test(char)) {
            index += 1
            continue
        }
        if (char === "'" || char === "\"") {
            const close = text.indexOf(char, index + 1)
            if (close === -1) {
                index += 1
                continue
            }
            pushMatch(matches, text.slice(index + 1, close), index + 1, cwd)
            index = close + 1
            continue
        }
        let end = index + 1
        while (end < text.length && !/\s/.test(text[end])) end += 1
        pushMatch(matches, text.slice(index, end), index, cwd)
        index = end
    }
    return matches
}

type TerminalTextCell = {
    x0: number
    y1: number
    width: number
}

function collectWrappedLine(
    host: TerminalTargetBufferHost,
    line1Based: number
): { text: string; cellsByOffset: TerminalTextCell[] } | null {
    const buffer = host.buffer.active
    const requested = line1Based - 1
    if (requested < 0 || requested >= buffer.length) return null
    if (!buffer.getLine(requested)) return null

    let startY0 = requested
    while (startY0 > 0) {
        const current = buffer.getLine(startY0)
        if (!current?.isWrapped) break
        startY0 -= 1
    }

    let text = ""
    const cellsByOffset: TerminalTextCell[] = []
    for (let y = startY0; y < buffer.length; y += 1) {
        const line = buffer.getLine(y)
        if (!line) break
        if (y > startY0 && !line.isWrapped) break

        const lineTextStart = text.length
        const maxCells = Math.min(host.cols, line.length)
        let trailingEmptyStart = text.length
        for (let x = 0; x < maxCells; x += 1) {
            const cell = line.getCell(x)
            if (!cell) break
            const width = cell.getWidth()
            if (width === 0) continue
            const chars = cell.getChars()
            const rendered = chars || " "
            if (chars) trailingEmptyStart = text.length + rendered.length
            text += rendered
            for (let offset = 0; offset < rendered.length; offset += 1) {
                cellsByOffset.push({ x0: x, y1: y + 1, width: Math.max(1, width) })
            }
        }

        // xterm exposes unoccupied cells as spaces. They are layout padding,
        // including the single null cell before a wide glyph wraps, and are
        // not part of the logical terminal token.
        if (trailingEmptyStart > lineTextStart && trailingEmptyStart < text.length) {
            text = text.slice(0, trailingEmptyStart)
            cellsByOffset.length = trailingEmptyStart
        } else if (trailingEmptyStart === lineTextStart) {
            text = text.slice(0, lineTextStart)
            cellsByOffset.length = lineTextStart
        }
    }
    if (!text) return null
    return { text, cellsByOffset }
}

function mapOffsetsToRange(
    cellsByOffset: TerminalTextCell[],
    start: number,
    end: number
): TerminalTargetLinkRange | null {
    const startCell = cellsByOffset[start]
    const endCell = cellsByOffset[Math.max(start, end - 1)]
    if (!startCell || !endCell) return null
    return {
        start: { x: startCell.x0 + 1, y: startCell.y1 },
        end: { x: endCell.x0 + endCell.width, y: endCell.y1 }
    }
}

export function resolveHerdrTerminalBaseCwd(input: {
    snapshot: HerdrSnapshot | null | undefined
    terminalId: string
    paneId?: string | null
    workspaceId?: string | null
}): string | null {
    const terminals = input.snapshot?.terminals ?? []
    const byTerminal = terminals.find((terminal) => terminal.terminalId === input.terminalId)
    if (!byTerminal) return null
    const byPane = input.paneId
        ? terminals.find(
            (terminal) =>
                terminal.paneId === input.paneId &&
                terminal.terminalId === input.terminalId
        )
        : undefined
    const exactCwd = [byPane?.cwd, byTerminal.cwd].find(isAbsolutePath)
    if (exactCwd) return exactCwd

    const spaceId = byTerminal.workspaceId ?? byPane?.workspaceId ?? input.workspaceId ?? null
    if (!spaceId) return null
    const space = input.snapshot?.spaces.find((item) => item.id === spaceId)
    return isAbsolutePath(space?.path) ? space.path : null
}

export async function openTerminalTarget(
    target: TerminalTarget,
    options: { triggerGroupIndex?: number } = {}
): Promise<void> {
    const actionLabel = i18n.t("targetOpen.action", { ns: "terminal" })
    if (target.kind === "url") {
        const workspace = useWorkspaceStore.getState().workspacePath
        if (!workspace) {
            await showActionError(actionLabel, i18n.t("targetOpen.noWorkspace", { ns: "terminal" }))
            return
        }
        const accepted = usePreviewStore.getState().navigate(workspace, target.url)
        if (!accepted) {
            await showActionError(
                actionLabel,
                i18n.t("targetOpen.previewRejected", { ns: "terminal" })
            )
            return
        }
        useWorkspaceStore.getState().openPreviewTab(options.triggerGroupIndex)
        return
    }

    const groupIndex = options.triggerGroupIndex ?? useWorkspaceStore.getState().activeGroupIndex
    try {
        await getDocument(target.path)
    } catch (error) {
        await showActionError(actionLabel, error)
        return
    }
    if (target.line === undefined) {
        useWorkspaceStore.getState().openTabInGroup(target.path, groupIndex)
    } else {
        useWorkspaceStore.getState().requestReveal(
            target.path,
            target.line,
            undefined,
            groupIndex
        )
    }
}

export function createTerminalTargetContextGate() {
    let pendingContextKey: string | null = null
    let contextHandledKey: string | null = null
    let hovered: TerminalTarget | null = null
    let lastKey: string | null = null
    let lastAt = 0

    function markConsumed(target: TerminalTarget): void {
        const key = targetKey(target)
        if (contextHandledKey === key) {
            contextHandledKey = null
            pendingContextKey = null
            return
        }
        pendingContextKey = key
    }

    function setHovered(target: TerminalTarget | null): void {
        hovered = target
    }

    function shouldDedup(target: TerminalTarget): boolean {
        const key = targetKey(target)
        const now = Date.now()
        if (lastKey === key && now - lastAt < 400) return true
        lastKey = key
        lastAt = now
        return false
    }

    function handleContextMenu(
        event: { preventDefault(): void; stopPropagation(): void } & Pick<
            MouseEvent,
            "button" | "metaKey" | "ctrlKey" | "altKey"
        >,
        onActivate: (target: TerminalTarget) => void,
        platform: { mac?: boolean } = {}
    ): boolean {
        const openGesture = isTerminalTargetOpenGesture(event, platform)
        const hoveredKey = hovered ? targetKey(hovered) : null
        if (pendingContextKey) {
            const consumesThisGesture = openGesture && hoveredKey === pendingContextKey
            pendingContextKey = null
            if (consumesThisGesture) {
                event.preventDefault()
                event.stopPropagation()
                return true
            }
        }
        if (hovered && openGesture) {
            event.preventDefault()
            event.stopPropagation()
            contextHandledKey = hoveredKey
            onActivate(hovered)
            return true
        }
        contextHandledKey = null
        return false
    }

    return {
        markConsumed,
        setHovered,
        shouldDedup,
        handleContextMenu,
        dispose() {
            pendingContextKey = null
            contextHandledKey = null
            hovered = null
            lastKey = null
        }
    }
}

export function createTerminalTargetLinkProvider(
    host: TerminalTargetBufferHost,
    options: {
        getCwd: () => string | null
        onActivate: (event: MouseEvent, target: TerminalTarget) => void
        onHover?: (target: TerminalTarget | null) => void
        isMac?: () => boolean
    }
): { provideLinks(bufferLineNumber: number, callback: (links: TerminalTargetLink[] | undefined) => void): void } {
    const platformOf = () => ({
        mac: options.isMac?.() ?? isMacPlatform()
    })
    const documentChecks = new Map<string, { checkedAt: number; openable: boolean }>()
    const pendingDocumentChecks = new Map<string, Promise<boolean>>()
    const DOCUMENT_CHECK_TTL_MS = 2_000
    const DOCUMENT_CHECK_CACHE_MAX = 256

    function checkOpenableFile(path: string): Promise<boolean> {
        const cached = documentChecks.get(path)
        if (cached && Date.now() - cached.checkedAt < DOCUMENT_CHECK_TTL_MS) {
            return Promise.resolve(cached.openable)
        }
        const pending = pendingDocumentChecks.get(path)
        if (pending) return pending
        const check = isOpenableFile(path)
            .catch(() => false)
            .then((openable) => {
                documentChecks.delete(path)
                documentChecks.set(path, { checkedAt: Date.now(), openable })
                if (documentChecks.size > DOCUMENT_CHECK_CACHE_MAX) {
                    const oldestPath = documentChecks.keys().next().value
                    if (oldestPath) documentChecks.delete(oldestPath)
                }
                return openable
            })
            .finally(() => pendingDocumentChecks.delete(path))
        pendingDocumentChecks.set(path, check)
        return check
    }

    return {
        async provideLinks(bufferLineNumber, callback) {
            const collected = collectWrappedLine(host, bufferLineNumber)
            if (!collected) {
                callback(undefined)
                return
            }
            const trimmed = collected.text.replace(/\s+$/, "")
            if (!trimmed) {
                callback(undefined)
                return
            }
            const cwd = options.getCwd()
            const matches = findTerminalTargetsInText(collected.text, cwd)
            if (matches.length === 0) {
                callback(undefined)
                return
            }
            const checkedMatches = await Promise.all(
                matches.map(async (match) => ({
                    match,
                    openable:
                        match.target.kind === "url" ||
                        await checkOpenableFile(match.target.path)
                }))
            )
            const latest = collectWrappedLine(host, bufferLineNumber)
            if (!latest || latest.text !== collected.text) {
                callback(undefined)
                return
            }
            const links: TerminalTargetLink[] = []
            for (const { match, openable } of checkedMatches) {
                if (!openable) continue
                const range = mapOffsetsToRange(
                    collected.cellsByOffset,
                    match.start,
                    match.end
                )
                if (!range) continue
                const link: TerminalTargetLink = {
                    range,
                    text: match.text,
                    decorations: {
                        pointerCursor: false,
                        underline: true
                    },
                    activate(event) {
                        if (!isTerminalTargetOpenGesture(event, platformOf())) return
                        options.onActivate(event, match.target)
                    },
                    hover(event) {
                        options.onHover?.(match.target)
                        const decorated = isTerminalTargetModifierHeld(event, platformOf())
                        if (link.decorations) {
                            link.decorations.pointerCursor = decorated
                            link.decorations.underline = true
                        }
                    },
                    leave() {
                        options.onHover?.(null)
                        if (link.decorations) {
                            link.decorations.pointerCursor = false
                            link.decorations.underline = true
                        }
                    }
                }
                links.push(link)
            }
            callback(links.length > 0 ? links : undefined)
        }
    }
}

export function installTerminalTargetOpen(
    terminal: Terminal,
    options: {
        getCwd: () => string | null
        isMac?: () => boolean
    }
): {
    dispose: () => void
    handleContextMenu: (
        event: { preventDefault(): void; stopPropagation(): void } & Pick<
            MouseEvent,
            "button" | "metaKey" | "ctrlKey" | "altKey"
        >
    ) => boolean
    resetHover: () => void
} {
    const gate = createTerminalTargetContextGate()
    const activate = (target: TerminalTarget) => {
        if (gate.shouldDedup(target)) return
        const triggerGroupIndex = useWorkspaceStore.getState().activeGroupIndex
        void openTerminalTarget(target, { triggerGroupIndex })
    }
    const provider = createTerminalTargetLinkProvider(terminal, {
        getCwd: options.getCwd,
        isMac: options.isMac,
        onActivate(event, target) {
            if (!isTerminalTargetOpenGesture(event, { mac: options.isMac?.() ?? isMacPlatform() })) {
                return
            }
            gate.markConsumed(target)
            activate(target)
        },
        onHover(target) {
            gate.setHovered(target)
        }
    })
    const disposable = terminal.registerLinkProvider(provider)
    const oscLinkHandler = {
        activate(event: MouseEvent, text: string) {
            if (!isTerminalTargetOpenGesture(event, { mac: options.isMac?.() ?? isMacPlatform() })) {
                return
            }
            const target = classifyTerminalTargetToken(text, options.getCwd())
            if (!target || target.kind !== "url") return
            gate.markConsumed(target)
            activate(target)
        },
        hover(event: MouseEvent, text: string) {
            const target = classifyTerminalTargetToken(text, options.getCwd())
            if (!target || target.kind !== "url") {
                gate.setHovered(null)
                return
            }
            gate.setHovered(target)
            const decorated = isTerminalTargetModifierHeld(event, {
                mac: options.isMac?.() ?? isMacPlatform()
            })
            const element = terminal.element
            if (element) element.style.cursor = decorated ? "pointer" : ""
        },
        leave() {
            gate.setHovered(null)
            const element = terminal.element
            if (element) element.style.cursor = ""
        },
        allowNonHttpProtocols: false
    }
    const previousLinkHandler = terminal.options.linkHandler ?? null
    terminal.options.linkHandler = oscLinkHandler

    return {
        dispose() {
            if (terminal.options.linkHandler === oscLinkHandler) {
                terminal.options.linkHandler = previousLinkHandler
            }
            const element = terminal.element
            if (element) element.style.cursor = ""
            disposable.dispose()
            gate.dispose()
        },
        resetHover() {
            gate.setHovered(null)
            const element = terminal.element
            if (element) element.style.cursor = ""
            if (terminal.rows > 0) terminal.refresh?.(0, terminal.rows - 1)
        },
        handleContextMenu(event) {
            return gate.handleContextMenu(
                event,
                activate,
                { mac: options.isMac?.() ?? isMacPlatform() }
            )
        }
    }
}
