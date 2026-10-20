import { useEffect, useState } from "react"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"

/**
 * Runs `load` once the socket is ready and again after every reconnect, a
 * change of `token` or a `reload()`. `data` and `error` are the latest outcome;
 * a dropped connection is not an error, the reconnect loads again. The previous
 * outcome stays on screen while a load is in flight unless `keepPrevious` is
 * false, for a caller whose old result must not outlive the question it answered.
 */
export function useLoad<T>(load: () => Promise<T>, token = 0, keepPrevious = true): { data: T | null; error: string | null; reload: () => void } {
  const ready = useCompanion((state) => state.socket === "ready")
  const epoch = useCompanion((state) => state.epoch)
  // `from` is what started the load that produced `data`.
  const [outcome, setOutcome] = useState<{ data: T | null; error: string | null; from: unknown[] }>({ data: null, error: null, from: [] })
  const [run, setRun] = useState(0)

  useEffect(() => {
    if (!ready) return
    let live = true
    const from = [load, epoch, run, token]
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
  }, [load, ready, epoch, run, token])

  const current = [load, epoch, run, token]
  return {
    data: keepPrevious || outcome.from.every((value, index) => value === current[index]) ? outcome.data : null,
    error: outcome.error,
    reload() {
      setOutcome((previous) => ({ ...previous, error: null }))
      setRun((value) => value + 1)
    },
  }
}
