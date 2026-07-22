import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"

import { ptyClose, ptyOpen, ptyResize, ptyWrite } from "../lib/ipc"
import type { PtyEvent } from "../lib/types"
import { installTerminalImePositioning } from "./terminalImePositioning"
import { registerTerminalView } from "./terminalViewRegistry"
import { buildXtermTheme } from "./xtermTheme"

export interface TerminalSessionProps {
    workspace: string
    sessionId: string
    shell?: string | null
    shellArgs?: string[]
    active: boolean
    visible?: boolean
    onExit?: (code: number | null) => void
    onTitleChange?: (title: string) => void
    onReady?: () => void
    onOpenError?: (message: string) => void
}

type TerminalMode = "light" | "dark"

const defaultCols = 80
const defaultRows = 24

function currentMode(): TerminalMode {
    return document.documentElement.classList.contains("dark") ? "dark" : "light"
}

function safeFit(fitAddon: FitAddon): void {
    try {
        fitAddon.fit()
    } catch {
        // xterm measures real layout here; jsdom and hidden panes may not have it.
    }
}

function safeFocus(term: Terminal): void {
    try {
        term.focus()
    } catch {
        // xterm focus touches DOM internals that are not available in jsdom.
    }
}

function terminalSize(term: Terminal): { cols: number; rows: number } {
    return {
        cols: term.cols || defaultCols,
        rows: term.rows || defaultRows
    }
}

function writeExitNotice(term: Terminal, code: number | null): void {
    const suffix = code === null ? "" : ` ${code}`
    term.write(`\r\n[Exited${suffix}]\r\n`)
}

export function TerminalSession({
    workspace,
    sessionId,
    shell = null,
    shellArgs,
    active,
    visible = true,
    onExit,
    onTitleChange,
    onReady,
    onOpenError
}: TerminalSessionProps) {
    const containerRef = useRef<HTMLDivElement | null>(null)
    const termRef = useRef<Terminal | null>(null)
    const fitRef = useRef<FitAddon | null>(null)
    const observerRef = useRef<ResizeObserver | null>(null)
    const themeObserverRef = useRef<MutationObserver | null>(null)
    const dataDisposableRef = useRef<{ dispose: () => void } | null>(null)
    const titleDisposableRef = useRef<{ dispose: () => void } | null>(null)
    const imePositioningRef = useRef<{ dispose: () => void } | null>(null)
    const capturedPasteWritesRef = useRef<Promise<void>[] | null>(null)
    const unregisterViewRef = useRef<(() => void) | null>(null)
    const openedRef = useRef(false)
    const disposedRef = useRef(false)
    const openPromiseRef = useRef<Promise<unknown> | null>(null)
    const openSettledRef = useRef(true)
    const openReadyRef = useRef(false)
    const cleanupTimerRef = useRef<number | null>(null)
    const lastSizeRef = useRef({ cols: defaultCols, rows: defaultRows })
    const onExitRef = useRef(onExit)
    const onTitleChangeRef = useRef(onTitleChange)
    const onReadyRef = useRef(onReady)
    const onOpenErrorRef = useRef(onOpenError)
    const visibleRef = useRef(visible)
    const previousVisibleRef = useRef(visible)
    const [exitCode, setExitCode] = useState<number | null | undefined>(undefined)

    useLayoutEffect(() => {
        visibleRef.current = visible
    }, [visible])

    useEffect(() => {
        onExitRef.current = onExit
        onTitleChangeRef.current = onTitleChange
        onReadyRef.current = onReady
        onOpenErrorRef.current = onOpenError
    }, [onExit, onOpenError, onReady, onTitleChange])

    useEffect(() => {
        function scheduleCleanup() {
            if (cleanupTimerRef.current !== null) return
            disposedRef.current = true
            openReadyRef.current = false

            cleanupTimerRef.current = window.setTimeout(() => {
                cleanupTimerRef.current = null
                const pendingOpen = openSettledRef.current ? null : openPromiseRef.current
                themeObserverRef.current?.disconnect()
                observerRef.current?.disconnect()
                dataDisposableRef.current?.dispose()
                titleDisposableRef.current?.dispose()
                imePositioningRef.current?.dispose()
                unregisterViewRef.current?.()
                fitRef.current?.dispose()
                termRef.current?.dispose()
                themeObserverRef.current = null
                observerRef.current = null
                dataDisposableRef.current = null
                titleDisposableRef.current = null
                imePositioningRef.current = null
                capturedPasteWritesRef.current = null
                unregisterViewRef.current = null
                fitRef.current = null
                termRef.current = null
                openedRef.current = false
                void ptyClose(sessionId).catch(() => undefined)
                if (pendingOpen) {
                    void pendingOpen
                        .then(() => ptyClose(sessionId).catch(() => undefined))
                        .catch(() => undefined)
                }
            }, 0)
        }

        if (cleanupTimerRef.current !== null) {
            window.clearTimeout(cleanupTimerRef.current)
            cleanupTimerRef.current = null
            disposedRef.current = false
        }

        if (openedRef.current) return scheduleCleanup

        const container = containerRef.current
        if (!container) return undefined

        const term = new Terminal({
            cols: defaultCols,
            rows: defaultRows,
            cursorBlink: true,
            fontFamily: "\"SFMono-Regular\", \"Cascadia Code\", \"JetBrains Mono\", Menlo, Consolas, monospace",
            fontSize: 12,
            scrollback: 4000,
            theme: buildXtermTheme(currentMode())
        })
        const fitAddon = new FitAddon()

        termRef.current = term
        fitRef.current = fitAddon
        openedRef.current = true
        disposedRef.current = false
        openReadyRef.current = false

        term.loadAddon(fitAddon)
        term.open(container)
        imePositioningRef.current = installTerminalImePositioning(term)
        const terminalView = {
            hasSelection: () => term.hasSelection(),
            getSelection: () => term.getSelection(),
            isReady: () => openReadyRef.current && !disposedRef.current,
            paste: async (text: string) => {
                const pendingOpen = openPromiseRef.current
                if (pendingOpen) await pendingOpen
                if (!openReadyRef.current || disposedRef.current) {
                    throw new Error("Terminal session is not ready")
                }
                const writes: Promise<void>[] = []
                capturedPasteWritesRef.current = writes
                try {
                    // Keep xterm's bracketed-paste transformation, but capture the
                    // resulting PTY write promise so the command can observe failure.
                    term.paste(text)
                } finally {
                    capturedPasteWritesRef.current = null
                }
                if (writes.length === 0) throw new Error("Terminal paste produced no input")
                await Promise.all(writes)
            },
            clear: () => term.clear()
        }
        unregisterViewRef.current = registerTerminalView(sessionId, terminalView)
        term.attachCustomKeyEventHandler((event) => {
            if (
                event.type !== "keydown"
                || event.altKey
                || (!event.ctrlKey && !event.metaKey)
            ) return true

            const key = event.key.toLowerCase()
            if (key === "c") {
                if (!term.hasSelection()) return true
                event.preventDefault()
                void writeText(term.getSelection()).catch(() => undefined)
                return false
            }
            if (key === "v") {
                event.preventDefault()
                if (!terminalView.isReady()) return false
                void readText()
                    .then((text) => {
                        if (text.length === 0 || !terminalView.isReady()) return undefined
                        return terminalView.paste(text)
                    })
                    .catch(() => undefined)
                return false
            }
            return true
        })
        titleDisposableRef.current = term.onTitleChange((title) => {
            if (!disposedRef.current) onTitleChangeRef.current?.(title)
        })
        if (visibleRef.current) safeFit(fitAddon)
        lastSizeRef.current = terminalSize(term)

        const dataDisposable = term.onData((data) => {
            if (disposedRef.current) return
            const write = ptyWrite(sessionId, data)
            const captured = capturedPasteWritesRef.current
            if (captured) captured.push(write)
            else void write.catch(() => undefined)
        })
        dataDisposableRef.current = dataDisposable

        const handleEvent = (event: PtyEvent) => {
            if (disposedRef.current) return
            if (event.type === "output") {
                term.write(event.data)
                return
            }

            writeExitNotice(term, event.code)
            setExitCode(event.code)
            onExitRef.current?.(event.code)
        }

        openSettledRef.current = false
        const openPromise = ptyOpen(
            workspace,
            sessionId,
            shell ?? null,
            shellArgs,
            lastSizeRef.current.cols,
            lastSizeRef.current.rows,
            handleEvent
        )
        openPromiseRef.current = openPromise
        void openPromise
            .then(() => {
                if (openPromiseRef.current === openPromise && !disposedRef.current) {
                    openReadyRef.current = true
                    onReadyRef.current?.()
                }
            })
            .catch((error) => {
                if (disposedRef.current) return
                const message = error instanceof Error ? error.message : String(error)
                term.write(`\r\n[Failed to open terminal: ${message}]\r\n`)
                onOpenErrorRef.current?.(message)
            })
            .finally(() => {
                if (openPromiseRef.current === openPromise) {
                    openSettledRef.current = true
                    openPromiseRef.current = null
                }
            })

        const resizeObserver = new ResizeObserver(() => {
            if (disposedRef.current || !visibleRef.current) return
            safeFit(fitAddon)
            const next = terminalSize(term)
            if (next.cols === lastSizeRef.current.cols && next.rows === lastSizeRef.current.rows) return

            lastSizeRef.current = next
            void ptyResize(sessionId, next.cols, next.rows).catch(() => undefined)
        })
        resizeObserver.observe(container)
        observerRef.current = resizeObserver

        const themeObserver = new MutationObserver(() => {
            if (disposedRef.current) return
            term.options.theme = { ...buildXtermTheme(currentMode()) }
        })
        themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] })
        themeObserverRef.current = themeObserver

        return scheduleCleanup
    }, [sessionId, shell, shellArgs, workspace])

    useEffect(() => {
        const term = termRef.current
        const fitAddon = fitRef.current
        const becameVisible = visible && !previousVisibleRef.current
        previousVisibleRef.current = visible
        if (!visible || !term || !fitAddon) return
        if (becameVisible) {
            safeFit(fitAddon)
            const next = terminalSize(term)
            if (
                openReadyRef.current
                && (next.cols !== lastSizeRef.current.cols || next.rows !== lastSizeRef.current.rows)
            ) {
                lastSizeRef.current = next
                void ptyResize(sessionId, next.cols, next.rows).catch(() => undefined)
            }
        }
        if (active) safeFocus(term)
    }, [active, sessionId, visible])

    return (
        <div
            className="relative h-full min-h-0 w-full overflow-hidden bg-(--term-bg) text-(--term-fg)"
            data-testid={`terminal-session-${sessionId}`}
            data-visible={String(visible)}
        >
            <div ref={containerRef} className="h-full min-h-0 w-full" />
            {exitCode !== undefined ? (
                <div
                    role="status"
                    className="pointer-events-none absolute bottom-2 right-2 rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[12px] text-(--term-fg2)"
                >
                    Exited{exitCode === null ? "" : ` ${exitCode}`}
                </div>
            ) : null}
        </div>
    )
}
