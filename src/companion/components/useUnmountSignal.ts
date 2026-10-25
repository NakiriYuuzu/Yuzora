import { useEffect, useRef } from "react"

/** The current mount's signal, aborted when the screen unmounts to end its background work. */
export function useUnmountSignal(): () => AbortSignal {
  const controller = useRef<AbortController | null>(null)
  useEffect(() => {
    const current = new AbortController()
    controller.current = current
    return () => current.abort(new Error("unmounted"))
  }, [])
  return () => controller.current?.signal ?? AbortSignal.abort()
}
