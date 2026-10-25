import { useEffect, useState } from "react"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"

/**
 * Runs `load` once the socket is ready and again after every reconnect, a
 * change of `token` or a `reload()`. `data` and `error` are the latest outcome;
 * a dropped connection is not an error, the reconnect loads again. The previous
 * outcome stays on screen while a load is in flight unless `keepPrevious` is
 * false, for a caller whose old result must not outlive the question it answered:
 * it then shows nothing from the moment the socket leaves "ready" until the new
 * result lands.
 */
export function useLoad<T>(load: () => Promise<T>, token: unknown = 0, keepPrevious = true): { data: T | null; error: string | null; reload: () => void } {
  const ready = useCompanion((state) => state.socket === "ready")
  const epoch = useCompanion((state) => state.epoch)
  // The socket is "ready" again before hello bumps the epoch, so neither `ready` nor `epoch` tells two connections apart: `connection` counts every time the socket enters or leaves "ready", and a load's stamp carries it.
  const [wasReady, setWasReady] = useState(ready)
  const [connection, setConnection] = useState(0)
  if (wasReady !== ready) {
    setWasReady(ready)
    setConnection((value) => value + 1)
  }
  // `from` is what started the load that produced `data`.
  const [outcome, setOutcome] = useState<{ data: T | null; error: string | null; from: unknown[] }>({ data: null, error: null, from: [] })
  const [run, setRun] = useState(0)

  useEffect(() => {
    if (!ready) return
    let live = true
    const from = [load, epoch, run, token, connection]
    load().then(
      (data) => live && setOutcome({ data, error: null, from }),
      (error: unknown) => {
        if (!live || (error instanceof RpcError && error.code === "disconnected")) return
        setOutcome((previous) => ({ ...previous, error: error instanceof RpcError ? error.code : "unknown" }))
      },
    )
    return () => {
      live = false
    }
  }, [load, ready, epoch, run, token, connection])

  const current = [load, epoch, run, token, connection]
  return {
    data: keepPrevious || outcome.from.every((value, index) => value === current[index]) ? outcome.data : null,
    error: outcome.error,
    reload() {
      setOutcome((previous) => ({ ...previous, error: null }))
      setRun((value) => value + 1)
    },
  }
}
