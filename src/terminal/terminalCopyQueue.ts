import type { CopyLineEnding } from './terminalCopyFormat'

interface CopyJob {
  owner: object
  text: string
  lineEnding: CopyLineEnding
  onError: (error: unknown) => void
}
interface QueueDependencies {
  format: (text: string, lineEnding: CopyLineEnding, signal: AbortSignal) => string | Promise<string>
  write: (text: string, isCurrent: () => boolean) => Promise<void>
}

/** One pipeline and one pending snapshot across all HERDR panes. Clipboard
 * writes cannot be aborted, so a newer write must wait for the previous one.
 */
export function createTerminalCopyQueue(dependencies: QueueDependencies) {
  let revision = 0
  let pending: (CopyJob & { revision: number }) | undefined
  let active: { job: CopyJob; controller: AbortController } | undefined
  let running = false
  async function drain() {
    if (running) return
    running = true
    try {
      while (pending) {
        const job = pending
        pending = undefined
        const controller = new AbortController()
        active = { job, controller }
        const current = () => !controller.signal.aborted && job.revision === revision
        try {
          const formatted = dependencies.format(job.text, job.lineEnding, controller.signal)
          const text = typeof formatted === 'string' ? formatted : await formatted
          if (text && current()) await dependencies.write(text, current)
        } catch (error) {
          if (current()) job.onError(error)
        } finally {
          active = undefined
        }
      }
    } finally {
      running = false
    }
  }
  return {
    copy(job: CopyJob) {
      if (!job.text || !/\S/.test(job.text)) return
      pending = { ...job, revision: ++revision }
      void drain()
    },
    cancel(owner: object) {
      if (pending?.owner === owner) pending = undefined
      if (active?.job.owner === owner) active.controller.abort()
    },
  }
}
