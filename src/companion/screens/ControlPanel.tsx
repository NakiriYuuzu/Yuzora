import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import type { Terminal } from "@xterm/xterm"
import { Hand, Send, ShieldCheck, TerminalSquare } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Textarea } from "@/components/ui/textarea"
import { call } from "../client"
import { ConfirmDialog } from "../components/ConfirmDialog"
import { ElevateDialog } from "../components/ElevateDialog"
import { KeyBar } from "../components/KeyBar"
import { useNow } from "../components/useNow"
import type { AgentRow } from "../lib/agents"
import { outcomeUnknown, rpcErrorKey } from "../lib/errors"
import { keyBytes } from "../lib/keys"
import type { AgentKey, ControlOpen } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { byteLength, screenText } from "../lib/screen"
import { formatCountdown, secondsUntil } from "../lib/time"
import { notify, useCompanion } from "../store"

const PROMPT_BYTES = 16384
const INPUT_BYTES = 4096

type Dialog = null | "elevate" | "resize" | "takeover" | { screen: string; send: () => Promise<void> }

/** Turns a refused write into a notice. Nothing here retries a write. */
function report(error: unknown): void {
  const code = error instanceof RpcError ? error.code : "unknown"
  if (code === "elevation-required" || code === "elevation-expired") useCompanion.setState({ elevation: null })
  notify({ tone: outcomeUnknown(code) ? "warning" : "error", key: rpcErrorKey(code) })
}

/**
 * Control after a passkey elevation: agent prompts and keys need no attach;
 * typing raw input needs a control stream (`terminal.control`), which may
 * reflow the desktop pane and asks before taking over from someone else.
 */
export function ControlPanel({ session, pane, agent, terminal, control, onControl, conflict, onConflictHandled }: {
  session: string
  pane: string
  agent: AgentRow | null
  terminal: Terminal | null
  control: ControlOpen | null
  onControl: (open: ControlOpen | null) => void
  /** The control stream closed because someone else owns the pane. */
  conflict: boolean
  onConflictHandled: () => void
}) {
  const { t } = useTranslation()
  const elevation = useCompanion((state) => state.elevation)
  const offset = useCompanion((state) => state.offset)
  const ready = useCompanion((state) => state.socket === "ready")
  const now = useNow()
  const [dialog, setDialog] = useState<Dialog>(null)
  const [busy, setBusy] = useState(false)
  const [text, setText] = useState("")
  const [direct, setDirect] = useState(false)
  const [enter, setEnter] = useState(true)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])
  const remaining = elevation?.scope === "control" ? secondsUntil(elevation.until, offset, now) : 0
  const streamRemaining = control ? secondsUntil(control.until, offset, now) : 0
  // HERDR refuses a new prompt while the agent waits for interactive input.
  const blocked = agent?.status === "blocked"
  const toAgent = !!agent && !blocked && !(direct && control)

  const run = async (method: string, params: Record<string, unknown>, done?: () => void) => {
    setBusy(true)
    try {
      await call(method, params)
      notify({ tone: "info", key: "control.sent" })
      done?.()
    } catch (error) {
      report(error)
    } finally {
      setBusy(false)
    }
  }

  // A blocked agent is asking for approval: show what it asks before any key
  // or text reaches it, and require a second tap.
  const guarded = (send: () => Promise<void>) => {
    if (agent?.status === "blocked") setDialog({ screen: screenText(terminal), send })
    else void send()
  }

  const sendKey = (key: AgentKey) => guarded(() => (control
    ? run("terminal.input", { stream: control.stream, text: keyBytes(key, terminal?.modes.applicationCursorKeysMode ?? false) })
    : run("pane.sendKeys", { session, target: pane, keys: [key] })))

  const sendText = () => {
    if (!text) return
    if (toAgent) {
      if (byteLength(text) > PROMPT_BYTES) return notify({ tone: "error", key: "control.tooLong", values: { bytes: PROMPT_BYTES } })
      return guarded(() => run("agent.prompt", { session, target: pane, text }, () => setText("")))
    }
    if (!control) return
    const value = enter ? `${text}\r` : text
    if (byteLength(value) > INPUT_BYTES) return notify({ tone: "error", key: "control.tooLong", values: { bytes: INPUT_BYTES } })
    guarded(() => run("terminal.input", { stream: control.stream, text: value }, () => setText("")))
  }

  const open = async (takeover: boolean) => {
    setBusy(true)
    try {
      const opened = await call<ControlOpen>("terminal.control", { session, target: pane, takeover, confirmResize: true })
      if (!mounted.current) {
        // The pane was left while the RPC ran: nobody can release this lease any more.
        void call("terminal.release", { stream: opened.stream }).catch(() => undefined)
        return
      }
      onControl(opened)
      setDirect(true)
      setDialog(null)
    } catch (error) {
      if (error instanceof RpcError && error.code === "owned-elsewhere" && !takeover) setDialog("takeover")
      else {
        report(error)
        setDialog(null)
      }
    } finally {
      setBusy(false)
    }
  }

  const release = async () => {
    if (!control) return
    setBusy(true)
    try {
      await call("terminal.release", { stream: control.stream })
    } catch {
      // Release only tightens: the stream is gone either way.
    } finally {
      onControl(null)
      setDirect(false)
      setBusy(false)
    }
  }

  return (
    <section aria-label={t("control.title")} className="flex flex-col gap-2 border-t pt-3">
      {remaining === 0 && !control ? (
        <div className="flex flex-col gap-2 px-3">
          <Button size="lg" className="h-11" disabled={!ready} onClick={() => setDialog("elevate")}>
            <ShieldCheck aria-hidden />
            {t("control.take")}
          </Button>
          <p className="text-xs text-muted-foreground">{t("control.takeHint")}</p>
        </div>
      ) : (
        <>
          <div className="flex items-center gap-2 px-3 text-sm">
            <span className="flex-1 text-muted-foreground" role="timer">
              {control ? t("control.directUntil", { time: formatCountdown(streamRemaining) }) : t("control.until", { time: formatCountdown(remaining) })}
            </span>
            {control ? (
              <Button variant="outline" className="h-11" disabled={busy} onClick={() => void release()}>{t("control.release")}</Button>
            ) : (
              <Button variant="outline" className="h-11" disabled={busy || !ready} onClick={() => setDialog("resize")}>
                <TerminalSquare aria-hidden />
                {t("control.direct")}
              </Button>
            )}
          </div>
          <KeyBar disabled={busy || !ready || (!control && !agent)} onKey={sendKey} />
          {blocked && !control && <p className="px-3 text-sm text-muted-foreground">{t("control.blockedHint")}</p>}
          {((agent && !blocked) || control) && (
            <form className="flex flex-col gap-2 px-3" onSubmit={(event) => { event.preventDefault(); sendText() }}>
              {agent && !blocked && control && (
                <RadioGroup aria-label={t("control.target")} value={direct ? "terminal" : "agent"} onValueChange={(value) => setDirect(value === "terminal")}
                  className="flex gap-1 rounded-lg bg-muted p-1">
                  {(["agent", "terminal"] as const).map((value) => (
                    <RadioGroupItem key={value} value={value}
                      className="aspect-auto h-10 w-auto flex-1 rounded-md border-0 text-sm font-medium text-muted-foreground shadow-none dark:bg-transparent data-[state=checked]:bg-background data-[state=checked]:text-foreground data-[state=checked]:shadow-sm dark:data-[state=checked]:bg-background [&_[data-slot=radio-group-indicator]]:hidden">
                      {value === "terminal" ? t("control.toTerminal") : t("control.toAgent")}
                    </RadioGroupItem>
                  ))}
                </RadioGroup>
              )}
              <Textarea value={text} onChange={(event) => setText(event.target.value)} rows={2} className="min-h-11 text-base"
                placeholder={toAgent ? t("control.promptPlaceholder") : t("control.inputPlaceholder")} aria-label={toAgent ? t("control.promptPlaceholder") : t("control.inputPlaceholder")} />
              <div className="flex items-center gap-3">
                {!toAgent && (
                  <label className="flex min-h-11 items-center gap-2 text-sm">
                    <Checkbox className="size-5" checked={enter} onCheckedChange={(value) => setEnter(value === true)} />
                    {t("control.appendEnter")}
                  </label>
                )}
                <Button type="submit" className="ml-auto h-11" disabled={busy || !ready || !text}>
                  <Send aria-hidden />
                  {toAgent ? t("control.sendToAgent") : t("control.send")}
                </Button>
              </div>
            </form>
          )}
        </>
      )}

      {dialog === "elevate" && (
        <ElevateDialog title={t("control.elevateTitle")} description={t("control.elevateBody")} confirmLabel={t("control.elevateButton")}
          onElevated={() => setDialog(null)} onCancel={() => setDialog(null)} />
      )}
      {dialog === "resize" && (
        <ConfirmDialog title={t("control.directTitle")} description={t("control.directBody")} confirmLabel={t("control.direct")} busy={busy}
          onConfirm={() => void open(false)} onCancel={() => setDialog(null)} />
      )}
      {(dialog === "takeover" || conflict) && (
        <ElevateDialog title={t("control.takeoverTitle")} description={t("control.takeoverBody")} confirmLabel={t("control.takeoverButton")}
          onElevated={() => {
            onConflictHandled()
            void open(true)
          }}
          onCancel={() => {
            onConflictHandled()
            setDialog(null)
          }} />
      )}
      {dialog && typeof dialog === "object" && (
        <ConfirmDialog title={t("control.approveTitle")} description={t("control.approveBody")} confirmLabel={t("control.approveButton")} busy={busy}
          onConfirm={() => {
            const send = dialog.send
            setDialog(null)
            void send()
          }}
          onCancel={() => setDialog(null)}>
          <ScrollArea orientation="both" className="max-h-[45dvh] rounded-md bg-[var(--term-bg)]">
            <pre className="p-2 font-mono text-xs leading-snug whitespace-pre text-[var(--term-fg)]">{dialog.screen || t("control.approveEmpty")}</pre>
          </ScrollArea>
          <p className="flex items-start gap-1.5 text-xs text-muted-foreground">
            <Hand className="mt-0.5 size-3.5 shrink-0" aria-hidden />
            {t("control.approveNoCwd")}
          </p>
        </ConfirmDialog>
      )}
    </section>
  )
}
