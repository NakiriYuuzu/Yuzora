import { describe, expect, it, vi } from 'vitest'
import { createTerminalCopyQueue } from './terminalCopyQueue'

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve() }
const job = (text: string, owner = {}) => ({ text, owner, lineEnding: 'lf' as const, onError: vi.fn() })

describe('global HERDR copy queue', () => {
  it('formats small copies and starts a single write immediately', async () => {
    const format = vi.fn((s: string) => s), write = vi.fn(async () => {})
    const queue = createTerminalCopyQueue({ format, write })
    queue.copy(job('a'))
    expect(format).toHaveBeenCalledOnce()
    expect(write).toHaveBeenCalledOnce()
    await flush()
  })
  it('discards old formatting and retains only the last of 200 cross-pane requests', async () => {
    const slow = deferred<string>()
    const format = vi.fn((s: string): string | Promise<string> => s === 'slow' ? slow.promise : s)
    const write = vi.fn(async () => {})
    const queue = createTerminalCopyQueue({ format, write })
    queue.copy(job('slow'))
    for (let i = 0; i < 200; i++) queue.copy(job(String(i)))
    expect(format).toHaveBeenCalledOnce()
    slow.resolve('obsolete')
    await flush()
    expect(format).toHaveBeenCalledTimes(2)
    expect(write).toHaveBeenCalledExactlyOnceWith('199', expect.any(Function))
  })
  it('serializes unabortable writes so the last selection remains in the clipboard', async () => {
    const first = deferred<void>(), writes: string[] = []
    const write = vi.fn(async (s: string) => { if (s === 'a') await first.promise; writes.push(s) })
    const queue = createTerminalCopyQueue({ format: s => s, write })
    queue.copy(job('a')); queue.copy(job('b')); queue.copy(job('c'))
    expect(write).toHaveBeenCalledOnce()
    first.resolve(); await flush()
    expect(writes).toEqual(['a', 'c'])
  })
  it('cancels only the disposed controller and never writes its delayed formatting', async () => {
    const slow = deferred<string>(), owner = {}, write = vi.fn(async () => {})
    let signal!: AbortSignal
    const queue = createTerminalCopyQueue({ format: (s, _, abort) => { signal = abort; return s === 'a' ? slow.promise : s }, write })
    queue.copy(job('a', owner)); queue.copy(job('b'))
    queue.cancel(owner)
    expect(signal.aborted).toBe(true)
    slow.resolve('a'); await flush()
    expect(write).toHaveBeenCalledExactlyOnceWith('b', expect.any(Function))
  })
  it('empty selections do not replace pending valid work or the clipboard', async () => {
    const write = vi.fn(async () => {})
    const queue = createTerminalCopyQueue({ format: s => s, write })
    queue.copy(job('a')); queue.copy(job(' \t\n')); queue.copy(job(''))
    await flush()
    expect(write).toHaveBeenCalledExactlyOnceWith('a', expect.any(Function))
  })
  it('reports current failures once, suppresses obsolete errors, and recovers', async () => {
    const slow = deferred<string>(), old = job('a'), next = job('b'), failed = job('c')
    const queue = createTerminalCopyQueue({ format: s => s === 'a' ? slow.promise : s, write: async s => { if (s === 'c') throw new Error('denied') } })
    queue.copy(old); queue.copy(next); slow.reject(new Error('worker failed')); await flush()
    expect(old.onError).not.toHaveBeenCalled()
    queue.copy(failed); await flush()
    expect(failed.onError).toHaveBeenCalledOnce()
    queue.copy(job('d')); await flush()
    expect(next.onError).not.toHaveBeenCalled()
  })
})
