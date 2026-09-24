import { useEffect, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { useTranslation } from "react-i18next"
import i18n from "@/lib/i18n"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { useHerdrNativeStore, type HerdrNativeSelection } from "@/state/herdrNativeStore"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { useHerdrStore } from "@/state/herdrStore"
import { invokeHerdr } from "@/lib/herdrProvider"
import { herdrFeature } from "@/lib/herdrFeatures"
import { herdrPaneFocus, herdrTerminalInput, herdrTerminalRelease, herdrTerminalResize } from "@/lib/herdrIpc"
import type { HerdrTerminalEvent, HerdrTerminalOpenResult } from "@/lib/herdrTypes"
import { terminalFontStack } from "@/terminal/terminalFonts"
import { buildXtermTheme } from "@/terminal/xtermTheme"
import { installTerminalClipboardHandling } from "@/terminal/terminalClipboard"
import { installKittyRenderer } from "@/terminal/kittyRenderer"
import "@xterm/xterm/css/xterm.css"

// A feature request belongs to one open() call; remounting the client must not replay it.
const dispatchedRequests = new WeakSet<HerdrNativeSelection>()

export default function HerdrNativeDialog({ selection }: { selection: HerdrNativeSelection }) {
  const { t } = useTranslation("herdrTools")
  const [container, setContainer] = useState<HTMLDivElement | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [ready, setReady] = useState(false)
  useEffect(() => {
    // Radix mounts portal content after the parent effect's first pass.
    const element = container
    if (!element) return
    let disposed = false, failed = false, id: string | null = null, queue = Promise.resolve(), queuedBytes = 0
    const settings = useTerminalSettingsStore.getState()
    const term = new Terminal({ fontFamily: terminalFontStack(settings.fontFamily), fontSize: settings.fontSize, allowProposedApi: true, allowTransparency: true, scrollback: 0, theme: buildXtermTheme(document.documentElement.classList.contains("dark") ? "dark" : "light") })
    const fit = new FitAddon()
    term.loadAddon(fit); term.open(element); fit.fit()
    const fail = (cause: unknown) => {
      if (disposed || failed) return
      failed = true; setReady(false); setError(String(cause)); term.options.disableStdin = true
      if (id) void herdrTerminalRelease(id).catch(() => undefined)
    }
    const pendingInput: string[] = []
    let inputQueue = Promise.resolve()
    const send = (text: string) => {
      if (disposed || failed) return
      if (id) inputQueue = inputQueue.then(async () => { if (!disposed && !failed && id) await herdrTerminalInput(id, text) }).catch(fail)
      else if (pendingInput.reduce((size, part) => size + part.length, 0) + text.length <= 16384) pendingInput.push(text)
    }
    const graphics = installKittyRenderer(term, send)
    const clipboard = installTerminalClipboardHandling(term, { canPaste: () => Boolean(id) && !failed, onCopyError: fail })
    // Official Copy mode exports its selected text via OSC 52, including over SSH.
    // Clipboard reads ("?") are deliberately not implemented.
    const osc = term.parser.registerOscHandler(52, data => {
      const encoded = data.slice(data.indexOf(";") + 1)
      if (encoded === "?" || encoded.length > 4 * 1024 * 1024) return true
      try { const bytes = Uint8Array.from(atob(encoded), char => char.charCodeAt(0)); void writeText(new TextDecoder().decode(bytes)).catch(cause => { if (!disposed) setError(String(cause)) }) } catch { /* malformed clipboard output */ }
      return true
    })
    const input = term.onData(send)
    let fitFrame = 0
    const resize = new ResizeObserver(() => {
      if (disposed || fitFrame) return
      fitFrame = requestAnimationFrame(() => {
        fitFrame = 0
        if (disposed) return
        fit.fit()
        if (id && !failed) void herdrTerminalResize(id, term.cols, term.rows).catch(fail)
      })
    })
    resize.observe(element)
    // Webfont measurement can change xterm's screen without resizing its host.
    const screen = term.element?.querySelector<HTMLElement>(".xterm-screen")
    if (screen) resize.observe(screen)
    const theme = new MutationObserver(() => { if (!disposed) term.options.theme = buildXtermTheme(document.documentElement.classList.contains("dark") ? "dark" : "light") })
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] })
    const onEvent = (event: HerdrTerminalEvent) => {
      if (disposed || failed) return
      if (event.type === "error") { fail(event.message); return }
      if (event.type === "closed") { fail(event.reason ?? i18n.t("herdrTools:nativeDetached")); return }
      if (event.type !== "frame") return
      const bytes = Uint8Array.from(atob(event.bytesBase64), char => char.charCodeAt(0))
      queuedBytes += bytes.length
      if (queuedBytes > 8 * 1024 * 1024) { fail(i18n.t("herdrTools:nativeOutputLimit")); return }
      queue = queue.then(async () => { if (!disposed && !failed) await graphics.write(bytes) }).catch(fail).finally(() => { queuedBytes -= bytes.length })
    }
    // Mount the dialog after the normal pane effects have released controllers.
    const open = requestAnimationFrame(() => {
      const screen = term.element?.querySelector<HTMLElement>(".xterm-screen")
      void invokeHerdr<HerdrTerminalOpenResult>("herdr_client_open", {
        sessionName: selection.sessionName,
        size: { cols: term.cols, rows: term.rows, cellWidth: Math.max(1, Math.round((screen?.clientWidth ?? element.clientWidth) / term.cols)), cellHeight: Math.max(1, Math.round((screen?.clientHeight ?? element.clientHeight) / term.rows)) }, onEvent
      }).then(async opened => {
        id = opened.sessionId
        if (disposed || failed) { await herdrTerminalRelease(id); return }
        // The first buffered write must own the same queue as later onData input.
        // Otherwise typing while this write awaits IPC can overtake it.
        if (pendingInput.length) send(pendingInput.splice(0).join(""))
        await inputQueue
        if (disposed || failed) return
        await herdrTerminalResize(id, term.cols, term.rows)
        if (selection.paneId) await herdrPaneFocus({ sessionName: selection.sessionName, paneId: selection.paneId })
        if (disposed) return
        setReady(true); clipboard.flushPendingPaste(); term.focus()
        if (selection.request && !dispatchedRequests.has(selection)) {
          dispatchedRequests.add(selection)
          const result = await herdrFeature(selection.sessionName, selection.request)
          const openedPane = result.plugin_pane as { pane?: { pane_id?: string } } | undefined
          if (!disposed && selection.request.method === "plugin.pane.open" && openedPane?.pane?.pane_id) {
            await herdrPaneFocus({ sessionName: selection.sessionName, paneId: openedPane.pane.pane_id })
          }
        }
      }).catch(fail)
    })
    return () => {
      disposed = true; cancelAnimationFrame(open); cancelAnimationFrame(fitFrame); resize.disconnect(); theme.disconnect()
      clipboard.dispose(); input.dispose(); osc.dispose(); graphics.dispose(); term.dispose()
      if (id) void herdrTerminalRelease(id).catch(() => undefined)
      void useHerdrStore.getState().bootstrap(selection.sessionName)
    }
  }, [container, selection])
  return <Dialog open onOpenChange={open => { if (!open) useHerdrNativeStore.getState().close() }}>
    <DialogContent className="flex h-[calc(100dvh-2rem)] max-h-[1000px] min-h-0 flex-col sm:max-w-[calc(100vw-2rem)]" onEscapeKeyDown={event => event.preventDefault()}>
      <DialogHeader className="shrink-0 pr-6"><DialogTitle>{t("nativeTitle")}</DialogTitle><DialogDescription>{t("nativeDescription")}</DialogDescription></DialogHeader>
      <p className="text-xs text-muted-foreground">{t("nativeCopyHelp")}</p>
      {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
      {!ready && !error && <p role="status">{t("nativeConnecting")}</p>}
      <div ref={setContainer} className="min-h-0 min-w-0 flex-1 overflow-hidden" aria-label={t("nativeTitle")} />
    </DialogContent>
  </Dialog>
}
