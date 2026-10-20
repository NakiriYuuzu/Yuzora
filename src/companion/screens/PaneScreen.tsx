import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { ArrowLeft, Maximize2, Minimize2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { call, listen, observe } from "../client"
import { ConnectionPill } from "../components/ConnectionPill"
import { Message, Page, Progress } from "../components/Page"
import { StatusBadge } from "../components/StatusBadge"
import { TerminalFrame } from "../components/TerminalFrame"
import { useTerminal } from "../components/useTerminal"
import { findAgent } from "../lib/agents"
import { rpcErrorKey } from "../lib/errors"
import type { ControlOpen } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { goBack, notify, useCompanion } from "../store"
import { ControlPanel } from "./ControlPanel"

type View = { stream: number; cols: number; rows: number }

export function PaneScreen({ session, pane }: { session: string; pane: string }) {
  const { t } = useTranslation()
  const ready = useCompanion((state) => state.socket === "ready")
  const epoch = useCompanion((state) => state.epoch)
  const snapshot = useCompanion((state) => state.snapshots[session])
  const agent = useMemo(() => findAgent(snapshot, pane), [snapshot, pane])
  const { terminal, mount } = useTerminal()
  const [fit, setFit] = useState(true)
  const [view, setView] = useState<View | null>(null)
  const [closed, setClosed] = useState<string | null>(null)
  const [reopen, setReopen] = useState(0)
  const [control, setControl] = useState<ControlOpen | null>(null)
  const [conflict, setConflict] = useState(false)
  const controlling = control !== null

  // Read-only observation while not controlling; reopened after every reconnect.
  useEffect(() => {
    if (!terminal || !ready || controlling) return
    let live = true
    let opened: number | null = null
    let unlisten = () => {}
    observe(session, pane).then(
      (open) => {
        if (!live) {
          void call("stream.close", { stream: open.stream }).catch(() => undefined)
          return
        }
        opened = open.stream
        terminal.reset()
        terminal.resize(open.cols, open.rows)
        unlisten = listen(open.stream, {
          data: (bytes) => terminal.write(bytes),
          closed: (reason) => live && setClosed(reason),
        })
        setClosed(null)
        setView(open)
      },
      (error: unknown) => live && setClosed(error instanceof RpcError ? error.code : "stream-closed"),
    )
    return () => {
      live = false
      unlisten()
      if (opened !== null) void call("stream.close", { stream: opened }).catch(() => undefined)
    }
  }, [terminal, ready, epoch, session, pane, reopen, controlling])

  // A control stream carries the pane's output itself and opens with a full
  // redraw, so the observed screen stays until it is repainted (the approval
  // check reads it). It is never reopened automatically once it closes.
  useEffect(() => {
    if (!terminal || !control) return
    terminal.resize(control.cols, control.rows)
    return listen(control.stream, {
      data: (bytes) => terminal.write(bytes),
      closed: (reason) => {
        setControl(null)
        // HERDR reports an owner conflict by closing the stream it just opened.
        if (reason === "owned-elsewhere") setConflict(true)
        else if (reason !== "disconnected") notify({ tone: "warning", key: rpcErrorKey(reason) })
      },
    })
  }, [terminal, control])

  return (
    <Page
      title={agent?.name ?? pane}
      leading={
        <Button variant="ghost" size="icon" className="size-11" aria-label={t("pane.back")} onClick={goBack}>
          <ArrowLeft aria-hidden />
        </Button>
      }
      trailing={
        <>
          <ConnectionPill />
          {agent && <StatusBadge status={agent.status} />}
        </>
      }
    >
      <div className="flex flex-1 flex-col gap-3 py-3">
        {closed && closed !== "disconnected" && (
          <div className="flex flex-col gap-2 px-3">
            <Message tone="warning">{t("pane.closed", { reason: t(rpcErrorKey(closed)) })}</Message>
            <Button variant="outline" className="h-11" disabled={!ready} onClick={() => setReopen((value) => value + 1)}>{t("pane.reopen")}</Button>
          </div>
        )}
        {!view && !control && !closed && <Progress text={t("pane.loading")} />}
        <div className="bg-[var(--term-bg)]">
          <TerminalFrame terminal={terminal} mount={mount} fit={fit} cols={(control ?? view)?.cols ?? 0} rows={(control ?? view)?.rows ?? 0} />
        </div>
        <div className="flex items-center justify-end gap-2 px-3">
          <Button variant="outline" className="h-11" onClick={() => setFit((value) => !value)}>
            {fit ? <Maximize2 aria-hidden /> : <Minimize2 aria-hidden />}
            {fit ? t("pane.actualSize") : t("pane.fitWidth")}
          </Button>
        </div>
        <ControlPanel session={session} pane={pane} agent={agent} terminal={terminal} control={control} onControl={setControl} conflict={conflict} onConflictHandled={() => setConflict(false)} />
      </div>
    </Page>
  )
}
