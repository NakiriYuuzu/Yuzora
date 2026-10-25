import { afterEach, describe, expect, it, vi } from 'vitest'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import type { Terminal } from '@xterm/xterm'

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  readText: vi.fn(),
  writeText: vi.fn(),
}))

import { installTerminalClipboardHandling } from './terminalClipboard'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve() }
const controllers: ReturnType<typeof installTerminalClipboardHandling>[] = []

function pane(text: string, read?: () => Promise<string>) {
  const state = { text, extended: Boolean(read), read: read ?? (async () => '') }
  const element = document.createElement('div')
  const term = {
    element,
    textarea: null,
    hasSelection: () => Boolean(state.text),
    getSelection: () => state.text,
    attachCustomKeyEventHandler: vi.fn(),
  } as unknown as Terminal
  const controller = installTerminalClipboardHandling(term, {
    extendedSelection: { active: () => state.extended, read: () => state.read() },
  })
  controllers.push(controller)
  return {
    state,
    dispose: controller.dispose,
    copy: () => element.dispatchEvent(new Event('copy', { cancelable: true })),
  }
}

afterEach(() => {
  for (const controller of controllers.splice(0)) controller.dispose()
  vi.mocked(writeText).mockReset()
  vi.restoreAllMocks()
})

describe('terminal copy request ordering before asynchronous selection reads', () => {
  it.each([
    ['same pane', false, false],
    ['same pane fallback', false, true],
    ['another pane', true, false],
    ['another pane fallback', true, true],
  ] as const)('does not overwrite a newer visible copy from %s', async (_, crossPane, failRead) => {
    vi.mocked(writeText).mockResolvedValue(undefined)
    const oldRead = deferred<string>()
    const old = pane('old visible fallback', () => oldRead.promise)
    old.copy()
    const latest = crossPane ? pane('new selection') : old
    latest.state.extended = false
    latest.state.text = 'new selection'
    latest.copy()
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('new selection')

    if (failRead) oldRead.reject(new Error('selection read failed'))
    else oldRead.resolve('old extended selection')
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('new selection')
  })

  it.each([false, true])('keeps request order when extended reads finish backwards (cross pane: %s)', async (crossPane) => {
    vi.mocked(writeText).mockResolvedValue(undefined)
    const first = deferred<string>(), second = deferred<string>()
    const old = pane('old visible', () => first.promise)
    old.copy()
    const latest = crossPane ? pane('new visible', () => second.promise) : old
    latest.state.read = () => second.promise
    latest.copy()
    second.resolve('new extended')
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('new extended')
    first.resolve('old extended')
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('new extended')
  })

  it('preserves the latest pending copy while a native write cannot be aborted', async () => {
    const writing = deferred<void>(), reading = deferred<string>(), writes: string[] = []
    vi.mocked(writeText).mockImplementation(async text => {
      if (text === 'writing') await writing.promise
      writes.push(text)
    })
    pane('writing').copy()
    pane('old visible', () => reading.promise).copy()
    pane('latest').copy()
    reading.resolve('old extended')
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('writing')
    writing.resolve()
    await flush()
    expect(writes).toEqual(['writing', 'latest'])
  })

  it.each([false, true])('does not discard valid pending text for a newer blank selection (extended: %s)', async (extended) => {
    vi.mocked(writeText).mockResolvedValue(undefined)
    const oldRead = deferred<string>()
    pane('valid visible', () => oldRead.promise).copy()
    pane(' \t\n', extended ? async () => ' \t\n' : undefined).copy()
    await flush()
    expect(writeText).not.toHaveBeenCalled()
    oldRead.resolve('valid extended')
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('valid extended')
  })

  it.each([false, true])('does not complete a disposed selection read (failure: %s)', async (failRead) => {
    vi.mocked(writeText).mockResolvedValue(undefined)
    const oldRead = deferred<string>()
    const old = pane('old fallback', () => oldRead.promise)
    old.copy()
    old.dispose()
    if (failRead) oldRead.reject(new Error('selection read failed'))
    else oldRead.resolve('old extended')
    await flush()
    expect(writeText).not.toHaveBeenCalled()
    pane('current').copy()
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith('current')
  })

  it.each([
    ['Macintosh', '\n'],
    ['Windows NT 10.0', '\r\n'],
  ])('preserves current fallback formatting and Unicode on %s', async (platform, eol) => {
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(platform)
    vi.mocked(writeText).mockResolvedValue(undefined)
    pane('  新的文字\r\n  🙂', async () => { throw new Error('read failed') }).copy()
    await flush()
    expect(writeText).toHaveBeenCalledExactlyOnceWith(`新的文字${eol}🙂`)
  })
})
