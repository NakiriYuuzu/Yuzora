import { beforeEach, expect, it, vi } from 'vitest'
vi.mock('@/lib/herdrIpc', () => ({ herdrTerminalOpen: vi.fn(), herdrTerminalInput: vi.fn(), herdrTerminalResize: vi.fn(), herdrTerminalScroll: vi.fn(), herdrTerminalRelease: vi.fn() }))
vi.mock('./herdrScrollIpc', () => ({ readPaneScroll: vi.fn(), setPaneScroll: vi.fn() }))
import { herdrTerminalOpen, herdrTerminalScroll } from '@/lib/herdrIpc'
import { readPaneScroll, setPaneScroll } from './herdrScrollIpc'
import { createHerdrTerminalTransport } from './terminalTransport'
const base = { offsetFromBottom: 0, maxOffsetFromBottom: 100, viewportRows: 24 }
beforeEach(() => {
  vi.resetAllMocks()
  vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: 's1', target: 't1', mode: 'control', role: 'controller', takeover: true, cols: 80, rows: 24 })
})
it.each(['null', 'error'])('reports unavailable WSL pane hydration (%s) without sending unsafe connector input', async outcome => {
  if (outcome === 'null') vi.mocked(readPaneScroll).mockResolvedValue(null)
  else vi.mocked(readPaneScroll).mockRejectedValue(new Error('host-request-timeout'))
  const t = createHerdrTerminalTransport({ terminalId: 't1', paneId: 'p1', sessionName: 'wsl', paneScrollEnabled: () => true, terminalScrollEnabled: () => false })
  await t.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  await expect(t.scroll!(-3)).rejects.toThrow()
  expect(herdrTerminalScroll).not.toHaveBeenCalled()
  t.detach()
})
it('does not retry a rejected connector when its pane fallback has no range', async () => {
  vi.mocked(herdrTerminalScroll).mockRejectedValue(new Error('unsupported'))
  vi.mocked(readPaneScroll).mockResolvedValue(null)
  const t = createHerdrTerminalTransport({ terminalId: 't1', paneId: 'p1', sessionName: 'default', paneScrollEnabled: () => true, terminalScrollEnabled: () => true })
  await t.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  await expect(t.scroll!(-3)).rejects.toThrow()
  await expect(t.scroll!(-3)).rejects.toThrow()
  expect(herdrTerminalScroll).toHaveBeenCalledTimes(1)
  t.detach()
})
it('renegotiates the connector scroll breaker after reopening', async () => {
  vi.mocked(herdrTerminalScroll).mockRejectedValueOnce(new Error('unsupported')).mockResolvedValue(undefined)
  const t = createHerdrTerminalTransport({ terminalId: 't1' })
  await t.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  await expect(t.scroll!(-3)).rejects.toThrow()
  t.detachSession()
  await t.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  await t.scroll!(-3)
  expect(herdrTerminalScroll).toHaveBeenCalledTimes(2)
  t.detach()
})
it('publishes successful pane position even when the mutation returns no metadata', async () => {
  vi.mocked(readPaneScroll).mockResolvedValue(base)
  vi.mocked(setPaneScroll).mockResolvedValue(null)
  const onPaneScroll = vi.fn()
  const t = createHerdrTerminalTransport({ terminalId: 't1', paneId: 'p1', sessionName: 'wsl', paneScrollEnabled: () => true, terminalScrollEnabled: () => false, onPaneScroll })
  await t.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  await t.scroll!(-3)
  expect(onPaneScroll).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 3 })
  t.detach()
})
