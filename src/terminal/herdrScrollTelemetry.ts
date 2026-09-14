/** Opt-in, content-free diagnostics; never log terminal bytes or command text. */
export interface HerdrScrollMetric {
  host?: string
  queueDepth?: number
  session: string
  pane: string
  serial: number
  phase: 'gesture' | 'dispatch' | 'ack' | 'next-frame' | 'error' | 'host-dispatch' | 'host-ack' | 'host-error'
  at: number
  pending: number
  attempt: number
  elapsedMs?: number
  errorClass?: string
}
let enabled = false
const samples: HerdrScrollMetric[] = []
export function enableHerdrScrollTelemetry(value: boolean) { enabled = value; samples.length = 0 }
export function readHerdrScrollTelemetry() { return samples.slice() }
export function recordHerdrScrollMetric(metric: HerdrScrollMetric) {
  if (!enabled) return
  if (samples.length === 512) samples.shift()
  samples.push(metric)
}
