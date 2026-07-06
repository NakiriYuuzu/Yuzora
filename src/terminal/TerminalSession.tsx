import { useEffect, useRef, useState } from "react"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"

import { ptyClose, ptyOpen, ptyResize, ptyWrite } from "../lib/ipc"
import type { PtyEvent } from "../lib/types"
import { buildXtermTheme } from "./xtermTheme"

export interface TerminalSessionProps {
    workspace: string
    sessionId: string
    shell?: string | null
    shellArgs?: string[]
    active: boolean
    onExit?: (code: number | null) => void
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
    onExit
}: TerminalSessionProps) {
    const containerRef = useRef<HTMLDivElement | null>(null)
    const termRef = useRef<Terminal | null>(null)
    const fitRef = useRef<FitAddon | null>(null)
    const observerRef = useRef<ResizeObserver | null>(null)
    const themeObserverRef = useRef<MutationObserver | null>(null)
    const dataDisposableRef = useRef<{ dispose: () => void } | null>(null)
    const openedRef = useRef(false)
    const disposedRef = useRef(false)
    const openPromiseRef = useRef<Promise<unknown> | null>(null)
    const openSettledRef = useRef(true)
    const cleanupTimerRef = useRef<number | null>(null)
    const lastSizeRef = useRef({ cols: defaultCols, rows: defaultRows })
    const onExitRef = useRef(onExit)
    const [exitCode, setExitCode] = useState<number | null | undefined>(undefined)

    useEffect(() => {
        onExitRef.current = onExit
    }, [onExit])

    useEffect(() => {
        function scheduleCleanup() {
            if (cleanupTimerRef.current !== null) return
            disposedRef.current = true

            cleanupTimerRef.current = window.setTimeout(() => {
                cleanupTimerRef.current = null
                const pendingOpen = openSettledRef.current ? null : openPromiseRef.current
                themeObserverRef.current?.disconnect()
                observerRef.current?.disconnect()
                dataDisposableRef.current?.dispose()
                fitRef.current?.dispose()
                termRef.current?.dispose()
                themeObserverRef.current = null
                observerRef.current = null
                dataDisposableRef.current = null
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

        term.loadAddon(fitAddon)
        term.open(container)
        safeFit(fitAddon)
        lastSizeRef.current = terminalSize(term)

        const dataDisposable = term.onData((data) => {
            if (disposedRef.current) return
            void ptyWrite(sessionId, data).catch(() => undefined)
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
            .catch((error) => {
                if (disposedRef.current) return
                const message = error instanceof Error ? error.message : String(error)
                term.write(`\r\n[Failed to open terminal: ${message}]\r\n`)
            })
            .finally(() => {
                if (openPromiseRef.current === openPromise) {
                    openSettledRef.current = true
                    openPromiseRef.current = null
                }
            })

        const resizeObserver = new ResizeObserver(() => {
            if (disposedRef.current) return
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

        if (active) safeFocus(term)

        return scheduleCleanup
    }, [active, sessionId, shell, shellArgs, workspace])

    useEffect(() => {
        if (!active || !termRef.current) return
        safeFocus(termRef.current)
    }, [active])

    return (
        <div
            className="relative h-full min-h-0 w-full overflow-hidden bg-(--term-bg) text-(--term-fg)"
            data-testid={`terminal-session-${sessionId}`}
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
