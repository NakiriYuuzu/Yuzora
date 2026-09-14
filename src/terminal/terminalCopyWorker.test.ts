import { afterEach, describe, expect, it, vi } from 'vitest'
import { COPY_WORKER_THRESHOLD, createTerminalCopyFormatter } from './terminalCopyWorker'

function workerStub() {
  return { onmessage: null, onerror: null, onmessageerror: null, terminate: vi.fn(), postMessage: vi.fn() } as unknown as Worker
}
const large = 'x'.repeat(COPY_WORKER_THRESHOLD + 1)
afterEach(() => vi.useRealTimers())
describe('copy formatter worker boundary', () => {
  it('formats small selections synchronously without creating a Worker', () => {
    const create = vi.fn(), formatter = createTerminalCopyFormatter(create)
    expect(formatter.format('  text', 'crlf', new AbortController().signal)).toBe('text')
    expect(formatter.format('x'.repeat(COPY_WORKER_THRESHOLD), 'lf', new AbortController().signal)).toHaveLength(COPY_WORKER_THRESHOLD)
    expect(create).not.toHaveBeenCalled()
  })
  it('sends the complete large selection and reuses one worker until disposal', async () => {
    const worker = workerStub(), create = vi.fn(() => worker), formatter = createTerminalCopyFormatter(create)
    for (let id = 1; id <= 2; id++) {
      const result = formatter.format(large, 'crlf', new AbortController().signal)
      expect(worker.postMessage).toHaveBeenLastCalledWith({ id, text: large, lineEnding: 'crlf' })
      worker.onmessage!({ data: { id, text: large } } as MessageEvent)
      await expect(result).resolves.toBe(large)
    }
    expect(create).toHaveBeenCalledOnce()
    formatter.dispose()
    expect(worker.terminate).toHaveBeenCalledOnce()
  })
  it.each(['error', 'messageerror', 'malformed', 'timeout', 'abort', 'dispose', 'post'])('rejects %s without returning unformatted text and can recreate the worker', async kind => {
    vi.useFakeTimers()
    const worker = workerStub(), create = vi.fn(() => worker), formatter = createTerminalCopyFormatter(create), abort = new AbortController()
    if (kind === 'post') vi.mocked(worker.postMessage).mockImplementationOnce(() => { throw new Error('clone') })
    const result = formatter.format(large, 'lf', abort.signal)
    const rejected = expect(result).rejects.toThrow(/clipboard-/)
    if (kind === 'error') worker.onerror!({ preventDefault() {} } as ErrorEvent)
    if (kind === 'messageerror') worker.onmessageerror!({} as MessageEvent)
    if (kind === 'malformed') worker.onmessage!({ data: { id: 0, text: 'bad' } } as MessageEvent)
    if (kind === 'timeout') await vi.advanceTimersByTimeAsync(30_000)
    if (kind === 'abort') abort.abort()
    if (kind === 'dispose') formatter.dispose()
    await rejected
    expect(worker.terminate).toHaveBeenCalledOnce()
    expect(vi.getTimerCount()).toBe(0)
    const retry = formatter.format(large, 'lf', new AbortController().signal)
    worker.onmessage!({ data: { id: 2, text: large } } as MessageEvent)
    await expect(retry).resolves.toBe(large)
    expect(create).toHaveBeenCalledTimes(2)
    formatter.dispose()
  })
})
