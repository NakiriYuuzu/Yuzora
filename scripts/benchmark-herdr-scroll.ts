/** Deterministic slow-host model through the production controller. This measures
 * client queueing only; native frame/paint acceptance is recorded separately. */
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
const modulePath = resolve(process.argv[2] ?? 'src/terminal/herdrScrollController.ts')
const { createPaneScrollController } = await import(pathToFileURL(modulePath).href)
const samples: number[] = []
let queued = 0, peak = 0, limitErrors = 0, calls = 0, offset = 0, tail = Promise.resolve()
const base = { offsetFromBottom: 0, maxOffsetFromBottom: 2000, viewportRows: 24 }
const read = async () => ({ ...base, offsetFromBottom: offset })
let visible = 0
const controller = createPaneScrollController({ read, allowed: () => true, change: (s: typeof base | null) => { if (s) visible = s.offsetFromBottom }, error: () => {}, write: (value: number) => {
  calls++
  if (queued >= 32) { limitErrors++; return Promise.reject('host-request-limit') }
  queued++; peak = Math.max(peak, queued)
  const work = tail.then(() => new Promise<typeof base>(done => setTimeout(() => { offset = value; queued--; done({ ...base, offsetFromBottom: offset }) }, 40)))
  tail = work.then(() => {})
  return work
} })
await controller.refresh()
const started = performance.now()
for (let i = 1; i <= 200; i++) {
  const at = performance.now(); controller.move(i); samples.push(performance.now() - at)
  await new Promise(r => setTimeout(r, 4))
}
const ended = performance.now()
while (performance.now() - ended < 3000 && (queued || offset !== 200)) await new Promise(r => setTimeout(r, 20))
const settled = performance.now()
controller.dispose()
samples.sort((a, b) => a - b)
console.log(JSON.stringify({ model: 'serialized 40ms RPC, 32 total admission slots, 200 gestures every 4ms', calls, peakAdmitted: peak, limitErrors, finalServerOffset: offset, visibleOffset: visible, inputP50Ms: samples[100], inputP95Ms: samples[190], gestureDurationMs: ended - started, settleAfterGestureMs: settled - ended }, null, 2))
