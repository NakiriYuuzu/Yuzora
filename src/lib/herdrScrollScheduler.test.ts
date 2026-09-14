import { expect, it, vi } from 'vitest'
import { createHerdrScrollScheduler } from './herdrScrollScheduler'
it('admits one request, preserves FIFO fairness, and cancels queued work before dispatch', async () => {
  const q = createHerdrScrollScheduler()
  let done!: (n: number) => void
  const a = q.run(() => new Promise<number>(r => { done = r }))
  const cancel = new AbortController(), skipped = vi.fn(async () => 2)
  const b = q.run(skipped, cancel.signal).catch(e => e.message)
  const c = q.run(async () => 3)
  await Promise.resolve(); expect(q.depth).toBe(3)
  cancel.abort(); expect(await b).toBe('scroll-cancelled'); expect(q.depth).toBe(2)
  done(1); expect(await a).toBe(1); expect(await c).toBe(3)
  expect(skipped).not.toHaveBeenCalled()
})
it('has a hard bound and recovers after a synchronous adapter error', async () => {
  const q = createHerdrScrollScheduler()
  let done!: () => void
  const first = q.run(() => new Promise<void>(r => { done = r }))
  const tasks = Array.from({ length: 32 }, () => q.run(async () => 1))
  await expect(q.run(async () => 2)).rejects.toThrow('host-request-limit')
  done(); await first; await Promise.all(tasks)
  await expect(q.run(() => { throw new Error('failed') })).rejects.toThrow('failed')
  expect(await q.run(async () => 4)).toBe(4)
})
