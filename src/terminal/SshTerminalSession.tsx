import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"

import { sshOpenShell, sshResize, sshWrite } from "@/lib/ipc"
import type { SshDataEvent, SshExitEvent } from "@/lib/types"
import { buildXtermTheme } from "./xtermTheme"

export interface SshTerminalSessionProps {
    sessionId: string
    active: boolean
    onExit?: () => void
}

type TerminalMode = "light" | "dark"

const defaultCols = 80
const defaultRows = 24

// ssh_open_shell must run exactly once per *successfully opened* session id: a
// remount (mode switch or active-host switch) must not spawn a second shell
// channel on the same live connection. The id is added synchronously before the
// async open so a StrictMode double-mount can't race two opens. An id leaves the
// set when its ssh://exit fires (the session is retired and never reopened) OR
// when the open attempt itself fails — so a later remount / reconnect can retry
// instead of being skipped forever.
const openedShells = new Set<string>()

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
    return { cols: term.cols || defaultCols, rows: term.rows || defaultRows }
}

/**
 * SSH terminal surface (FEAT-2). A sibling of TerminalSession — same xterm
 * setup and theme so the two read identically — but driven by the ssh_* IPC and
 * the "ssh://data"/"ssh://exit" events instead of the pty channel. It never
 * disconnects the SSH session on unmount: the connection's lifecycle is owned by
 * sshStore, so switching modes/hosts keeps the remote session alive.
 */
export function SshTerminalSession({ sessionId, active, onExit }: SshTerminalSessionProps) {
    const containerRef = useRef<HTMLDivElement | null>(null)
    const termRef = useRef<Terminal | null>(null)
    const lastSizeRef = useRef({ cols: defaultCols, rows: defaultRows })
    const onExitRef = useRef(onExit)
    const activeRef = useRef(active)
    const [exited, setExited] = useState(false)

    useEffect(() => {
        onExitRef.current = onExit
    }, [onExit])

    useEffect(() => {
        activeRef.current = active
    }, [active])

    useEffect(() => {
        const container = containerRef.current
        if (!container) return undefined

        let disposed = false
        const term = new Terminal({
            cols: defaultCols,
            rows: defaultRows,
            cursorBlink: true,
            fontFamily:
                "\"SFMono-Regular\", \"Cascadia Code\", \"JetBrains Mono\", Menlo, Consolas, monospace",
            fontSize: 12,
            scrollback: 4000,
            theme: buildXtermTheme(currentMode())
        })
        const fitAddon = new FitAddon()
        termRef.current = term
        term.loadAddon(fitAddon)
        term.open(container)
        safeFit(fitAddon)
        lastSizeRef.current = terminalSize(term)

        const dataDisposable = term.onData((data) => {
            if (disposed) return
            void sshWrite(sessionId, data).catch(() => undefined)
        })

        const unlistenPromises = [
            listen<SshDataEvent>("ssh://data", (e) => {
                if (disposed || e.payload.sessionId !== sessionId) return
                term.write(e.payload.chunk)
            }),
            listen<SshExitEvent>("ssh://exit", (e) => {
                if (e.payload.sessionId !== sessionId) return
                openedShells.delete(sessionId)
                if (disposed) return
                term.write("\r\n[Disconnected]\r\n")
                setExited(true)
                onExitRef.current?.()
            })
        ]

        // Attach listeners first, then open the shell — a fresh shell's first
        // output arrives after a network round-trip, well after listen() settles.
        if (!openedShells.has(sessionId)) {
            openedShells.add(sessionId)
            void sshOpenShell(sessionId, lastSizeRef.current.cols, lastSizeRef.current.rows).catch(
                (error) => {
                    // The shell never opened — drop the marker so a later remount
                    // (mode switch, host switch, or the reconnect banner) retries
                    // instead of being skipped forever by the once-per-session guard.
                    openedShells.delete(sessionId)
                    if (disposed) return
                    const message = error instanceof Error ? error.message : String(error)
                    term.write(`\r\n[Failed to open shell: ${message}]\r\n`)
                }
            )
        }

        const resizeObserver = new ResizeObserver(() => {
            if (disposed) return
            safeFit(fitAddon)
            const next = terminalSize(term)
            if (next.cols === lastSizeRef.current.cols && next.rows === lastSizeRef.current.rows) return
            lastSizeRef.current = next
            void sshResize(sessionId, next.cols, next.rows).catch(() => undefined)
        })
        resizeObserver.observe(container)

        const themeObserver = new MutationObserver(() => {
            if (disposed) return
            term.options.theme = { ...buildXtermTheme(currentMode()) }
        })
        themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] })

        if (activeRef.current) safeFocus(term)

        return () => {
            disposed = true
            themeObserver.disconnect()
            resizeObserver.disconnect()
            dataDisposable.dispose()
            for (const p of unlistenPromises) void p.then((fn) => fn()).catch(() => undefined)
            fitAddon.dispose()
            term.dispose()
            termRef.current = null
        }
    }, [sessionId])

    useEffect(() => {
        if (!active || !termRef.current) return
        safeFocus(termRef.current)
    }, [active])

    return (
        <div
            className="relative h-full min-h-0 w-full overflow-hidden bg-(--term-bg) text-(--term-fg)"
            data-testid={`ssh-terminal-session-${sessionId}`}
        >
            <div ref={containerRef} className="h-full min-h-0 w-full" />
            {exited ? (
                <div
                    role="status"
                    className="pointer-events-none absolute right-2 bottom-2 rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[12px] text-(--term-fg2)"
                >
                    Disconnected
                </div>
            ) : null}
        </div>
    )
}
