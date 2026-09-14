import { formatTerminalSelection } from '../src/terminal/terminalCopyFormat'
import { createTerminalCopyFormatter } from '../src/terminal/terminalCopyWorker'

const run = document.querySelector<HTMLButtonElement>('#run')!
const output = document.querySelector<HTMLPreElement>('#result')!
run.onclick = async () => {
  run.disabled = true
  output.textContent = 'Running…'
  const formatter = createTerminalCopyFormatter()
  const unit = '  COPY 中文 fixture\n\n\n  next paragraph\n\n- parent\n  - child\n\n```py\n  run()\n```\n\n'
  const results = []
  const frame = () => new Promise<number>(resolve => requestAnimationFrame(resolve))
  const stats = (values: number[]) => {
    values.sort((a, b) => a - b)
    return { p50: +values[25].toFixed(2), p95: +values[47].toFixed(2), max: +values[49].toFixed(2) }
  }
  try {
    for (const size of [10 * 1024, 100 * 1024, 1024 * 1024]) {
      const text = unit.repeat(Math.ceil(size / unit.length)).slice(0, size)
      const expected = formatTerminalSelection(text, 'lf')
      // Expected-output construction is outside the measured main-thread window.
      await frame(); await frame()
      const dispatch: number[] = [], elapsed: number[] = [], longTasks: number[] = []
      let previous = 0, maxFrameGap = 0, frameId = 0
      const heartbeat = (time: number) => { if (previous) maxFrameGap = Math.max(maxFrameGap, time - previous); previous = time; frameId = requestAnimationFrame(heartbeat) }
      frameId = requestAnimationFrame(heartbeat)
      const supported = PerformanceObserver.supportedEntryTypes.includes('longtask')
      const observer = supported ? new PerformanceObserver(list => longTasks.push(...list.getEntries().map(entry => entry.duration))) : null
      observer?.observe({ type: 'longtask' })
      try {
        for (let i = 0; i < 50; i++) {
          const start = performance.now()
          const pending = formatter.format(text, 'lf', new AbortController().signal)
          dispatch.push(performance.now() - start)
          const result = await pending
          elapsed.push(performance.now() - start)
          if (result !== expected) throw new Error(`Output mismatch at ${size}`)
          await frame()
        }
        await frame()
        results.push({ codeUnits: size, outputCodeUnits: expected.length, correct: true, samples: 50, dispatchMs: stats(dispatch), resultMs: stats(elapsed), maxFrameGapMs: +maxFrameGap.toFixed(2), longTasks: supported ? longTasks : 'unsupported' })
      } finally { cancelAnimationFrame(frameId); observer?.disconnect() }
    }
    output.textContent = JSON.stringify({ status: 'PASS', note: 'Browser Worker and frame heartbeat; no native clipboard IPC', userAgent: navigator.userAgent, results }, null, 2)
  } catch (error) { output.textContent = `FAIL: ${String(error)}` }
  finally { formatter.dispose(); run.disabled = false }
}
