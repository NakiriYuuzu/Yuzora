import { useEffect, useRef, useState } from "react"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { toast } from "sonner"
import { useTranslation } from "react-i18next"
import i18n from "@/lib/i18n"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { herdrTerminalInput, herdrTerminalRelease, herdrTerminalResize } from "@/lib/herdrIpc"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import { machinesInteractiveOpen } from "@/lib/machinesIpc"
import { describeMachineError } from "@/lib/machinesErrors"
import { useMachinesInteractiveStore, type MachineInteractiveSelection } from "@/state/machinesInteractiveStore"
import { useMachinesStore } from "@/state/machinesStore"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { terminalFontStack } from "@/terminal/terminalFonts"
import { buildXtermTheme, xtermMinimumContrastRatio } from "@/terminal/xtermTheme"
import { installTerminalClipboardHandling } from "@/terminal/terminalClipboard"
import { installKittyRenderer } from "@/terminal/kittyRenderer"
import "@xterm/xterm/css/xterm.css"

/**
 * Hosts an official `herdr machine add|reconnect` or `herdr client` PTY. It never touches
 * panes, features or the herdr store: it only carries the official interactive client.
 */
export default function MachineInteractiveDialog({ selection }: { selection: MachineInteractiveSelection }) {
  const { t } = useTranslation("machines")
  const [container, setContainer] = useState<HTMLDivElement | null>(null)
  const [error, setError] = useState<{ message: string; detail: string | null } | null>(null)
  const [ready, setReady] = useState(false)
  const [ended, setEnded] = useState(false)
  const idsBefore = useRef<Set<string> | null>(null)
  if (idsBefore.current === null) idsBefore.current = new Set(useMachinesStore.getState().machines.map(machine => machine.id))
  const { spec } = selection

  useEffect(() => {
    const element = container
    if (!element) return
    let disposed = false, failed = false, id: string | null = null, queue = Promise.resolve(), queuedBytes = 0
    const settings = useTerminalSettingsStore.getState()
    const mode = () => (document.documentElement.classList.contains("dark") ? "dark" : "light")
    const term = new Terminal({ fontFamily: terminalFontStack(settings.fontFamily), fontSize: settings.fontSize, allowProposedApi: true, allowTransparency: true, scrollback: 0, theme: buildXtermTheme(mode()), minimumContrastRatio: xtermMinimumContrastRatio(mode()) })
    const fit = new FitAddon()
    term.loadAddon(fit); term.open(element); fit.fit()
    const fail = (cause: unknown) => {
      if (disposed || failed) return
      failed = true; setReady(false); term.options.disableStdin = true
      setError(describeMachineError(cause, (key, options) => i18n.t(key, { ns: "machines", ...options }) as string))
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
    // Official Copy mode exports its selection via OSC 52; clipboard reads ("?") are not implemented.
    const osc = term.parser.registerOscHandler(52, data => {
      const encoded = data.slice(data.indexOf(";") + 1)
      if (encoded === "?" || encoded.length > 4 * 1024 * 1024) return true
      try {
        const decoded = atob(encoded)
        const bytes = new Uint8Array(decoded.length)
        for (let index = 0; index < decoded.length; index++) bytes[index] = decoded.charCodeAt(index)
        void writeText(new TextDecoder().decode(bytes)).catch(cause => { if (!disposed) fail(cause) })
      } catch { /* malformed clipboard output */ }
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
    const screen = term.element?.querySelector<HTMLElement>(".xterm-screen")
    if (screen) resize.observe(screen)
    const theme = new MutationObserver(() => { if (disposed) return; term.options.theme = buildXtermTheme(mode()); term.options.minimumContrastRatio = xtermMinimumContrastRatio(mode()) })
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] })
    const onEvent = (event: HerdrTerminalEvent) => {
      if (disposed || failed) return
      if (event.type === "error") { fail(event.message); return }
      if (event.type === "closed") {
        // The official process ended: the outcome is decided by re-reading the machine list.
        failed = true; setReady(false); setEnded(true); term.options.disableStdin = true
        if (id) void herdrTerminalRelease(id).catch(() => undefined)
        return
      }
      if (event.type !== "frame") return
      const bytes = Uint8Array.from(atob(event.bytesBase64), char => char.charCodeAt(0))
      queuedBytes += bytes.length
      if (queuedBytes > 8 * 1024 * 1024) { fail("machines-output-too-large"); return }
      queue = queue.then(async () => { if (!disposed && !failed) await graphics.write(bytes) }).catch(fail).finally(() => { queuedBytes -= bytes.length })
    }
    const open = requestAnimationFrame(() => {
      const inner = term.element?.querySelector<HTMLElement>(".xterm-screen")
      void machinesInteractiveOpen(spec, {
        cols: term.cols, rows: term.rows,
        cellWidth: Math.max(1, Math.round((inner?.clientWidth ?? element.clientWidth) / term.cols)),
        cellHeight: Math.max(1, Math.round((inner?.clientHeight ?? element.clientHeight) / term.rows))
      }, onEvent).then(async opened => {
        id = opened.sessionId
        if (disposed || failed) { await herdrTerminalRelease(id); return }
        if (pendingInput.length) send(pendingInput.splice(0).join(""))
        await inputQueue
        if (disposed || failed) return
        await herdrTerminalResize(id, term.cols, term.rows)
        if (disposed) return
        setReady(true); clipboard.flushPendingPaste(); term.focus()
      }).catch(fail)
    })
    return () => {
      disposed = true; cancelAnimationFrame(open); cancelAnimationFrame(fitFrame); resize.disconnect(); theme.disconnect()
      clipboard.dispose(); input.dispose(); osc.dispose(); graphics.dispose(); term.dispose()
      if (id) void herdrTerminalRelease(id).catch(() => undefined)
    }
  }, [container, spec])

  const close = () => {
    useMachinesInteractiveStore.getState().close()
    const store = useMachinesStore.getState()
    if (spec.kind === "client") {
      // The official client may have completed auth / host-key confirmation: force a round to clear auth blocks.
      store.requestRefresh(true)
      return
    }
    void store.refreshList().then(machines => {
      if (spec.kind === "reconnect") {
        useMachinesStore.getState().requestRefresh()
        return
      }
      if (!machines) { toast.warning(t("interactive.unconfirmed")); return }
      const added = machines.some(machine => !idsBefore.current?.has(machine.id) && machine.target === spec.target)
      if (added) toast.success(t("interactive.saved", { target: spec.target }))
      else toast.warning(t("interactive.notSaved"))
    })
  }
  const title = spec.kind === "add" ? t("interactive.addTitle")
    : spec.kind === "reconnect" ? t("interactive.reconnectTitle", { label: selection.machineLabel ?? "" })
      : selection.machineLabel ? t("interactive.clientSelectTitle", { label: selection.machineLabel }) : t("interactive.clientTitle")
  return <Dialog open onOpenChange={open => { if (!open) close() }}>
    <DialogContent className="flex h-[calc(100dvh-2rem)] max-h-[1000px] min-h-0 flex-col sm:max-w-[calc(100vw-2rem)]" onEscapeKeyDown={event => event.preventDefault()}>
      <DialogHeader className="shrink-0 pr-6">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{spec.kind === "client" && selection.machineLabel
          ? t("interactive.clientHint", { label: selection.machineLabel }) : t("interactive.hint")}</DialogDescription>
      </DialogHeader>
      {error && <Alert variant="destructive"><AlertDescription>
        {error.message}
        {error.detail && <details className="mt-1 text-xs"><summary>{t("panel.diagnostics")}</summary><pre className="whitespace-pre-wrap [overflow-wrap:anywhere]">{error.detail}</pre></details>}
      </AlertDescription></Alert>}
      {!ready && !error && !ended && <p role="status">{t("interactive.connecting")}</p>}
      {ended && <p role="status">{t("interactive.ended")}</p>}
      <div ref={setContainer} className="min-h-0 min-w-0 flex-1 overflow-hidden" aria-label={title} />
      {(ended || error) && <DialogFooter><Button onClick={close}>{t("interactive.close")}</Button></DialogFooter>}
    </DialogContent>
  </Dialog>
}
