import { formatTerminalSelection, type CopyLineEnding } from './terminalCopyFormat'

export const COPY_WORKER_THRESHOLD = 64 * 1024

/** Used by the serial copy scheduler: at most one format call at a time. */
export function createTerminalCopyFormatter(createWorker = () => new Worker(new URL('./terminalCopy.worker.ts', import.meta.url), { type: 'module' })) {
  let worker: Worker | undefined
  let serial = 0
  let cancel: (() => void) | undefined
  function dispose() {
    cancel?.()
    worker?.terminate()
    worker = undefined
  }
  return {
    dispose,
    format(text: string, lineEnding: CopyLineEnding, signal: AbortSignal): string | Promise<string> {
      if (signal.aborted) throw new Error('clipboard-copy-cancelled')
      if (text.length <= COPY_WORKER_THRESHOLD) return formatTerminalSelection(text, lineEnding)
      if (cancel) throw new Error('clipboard-formatter-busy')
      worker ??= createWorker()
      const target = worker
      const id = ++serial
      return new Promise<string>((resolve, reject) => {
        const cleanup = () => {
          clearTimeout(timeout)
          target.onmessage = null
          target.onerror = null
          target.onmessageerror = null
          signal.removeEventListener('abort', abort)
          cancel = undefined
        }
        const fail = (reason: string) => {
          cleanup()
          target.terminate()
          if (worker === target) worker = undefined
          reject(new Error(reason))
        }
        const abort = () => fail('clipboard-copy-cancelled')
        const timeout = setTimeout(() => fail('clipboard-format-timeout'), 30_000)
        cancel = abort
        signal.addEventListener('abort', abort, { once: true })
        target.onerror = event => { event.preventDefault(); fail('clipboard-format-failed') }
        target.onmessageerror = () => fail('clipboard-format-failed')
        target.onmessage = (event: MessageEvent<unknown>) => {
          const data = event.data as { id?: number; text?: unknown }
          if (!data || data.id !== id || typeof data.text !== 'string') { fail('clipboard-format-failed'); return }
          cleanup()
          resolve(data.text)
        }
        try { target.postMessage({ id, text, lineEnding }) }
        catch { fail('clipboard-format-failed') }
      })
    },
  }
}
