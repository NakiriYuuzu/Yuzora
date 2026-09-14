/** bun scripts/benchmark-herdr-copy.ts
 * Bun formatter/Worker microbenchmark, not native WebView paint or clipboard IPC.
 */
import { formatTerminalSelection } from '../src/terminal/terminalCopyFormat'
import { createTerminalCopyFormatter } from '../src/terminal/terminalCopyWorker'

// Exact cleanup algorithm from the pre-change 4e6dc6a clipboard formatter.
function baseline(selection: string) {
  const lines = selection.replace(/\r\n?/g, '\n').split('\n').map(line => line.replace(/[ \t]+$/g, ''))
  let start = 0, end = lines.length
  while (start < end && lines[start].length === 0) start++
  while (end > start && lines[end - 1].length === 0) end--
  return lines.slice(start, end).join('\n')
}
const unit = '  第一段：copy fixture with unchanged  internal spaces.\n\n\n   Next paragraph.\n\n- list\n  - child\n\n```py\n  if ok:\n    print("copied")\n```\n\n'
const formatter = createTerminalCopyFormatter()
const samples = 50
function stats(values: number[]) {
  const ordered = values.toSorted((a, b) => a - b)
  return { p50: +ordered[Math.floor(ordered.length * .5)].toFixed(3), p95: +ordered[Math.ceil(ordered.length * .95) - 1].toFixed(3), max: +ordered.at(-1)!.toFixed(3) }
}
const results = []
try {
  for (const codeUnits of [10 * 1024, 100 * 1024, 1024 * 1024]) {
    const fixture = unit.repeat(Math.ceil(codeUnits / unit.length)).slice(0, codeUnits)
    const expected = formatTerminalSelection(fixture, 'lf')
    const oldTimes: number[] = [], formatTimes: number[] = [], dispatchTimes: number[] = [], elapsedTimes: number[] = []
    // Warm parser and worker once; cold worker latency is captured separately.
    const coldStart = performance.now()
    await formatter.format(fixture, 'lf', new AbortController().signal)
    const coldMs = performance.now() - coldStart
    for (let i = 0; i < samples; i++) {
      let start = performance.now(); baseline(fixture); oldTimes.push(performance.now() - start)
      start = performance.now(); formatTerminalSelection(fixture, 'lf'); formatTimes.push(performance.now() - start)
      start = performance.now()
      const result = formatter.format(fixture, 'lf', new AbortController().signal)
      dispatchTimes.push(performance.now() - start)
      const output = await result
      elapsedTimes.push(performance.now() - start)
      if (output !== expected) throw new Error('Worker output differs from synchronous formatter')
    }
    results.push({ codeUnits, samples, coldDispatchToResultMs: +coldMs.toFixed(3), outputCodeUnits: expected.length, baselineMs: stats(oldTimes), synchronousNewFormatMs: stats(formatTimes), callerDispatchMs: stats(dispatchTimes), dispatchToResultMs: stats(elapsedTimes) })
  }
  console.log(JSON.stringify({ runtime: `Bun ${Bun.version}`, platform: process.platform, arch: process.arch, baseline: '4e6dc6a', units: 'milliseconds', note: 'No native clipboard IPC or WebView paint is measured.', results }, null, 2))
} finally { formatter.dispose() }
