import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createElement } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import script from '../../src-tauri/src/preview_interaction.js?raw'
import { dispatchAppShortcut, tabShortcutBindings, useKeyboardSettingsStore } from '@/state/keyboardSettingsStore'
import type { PreviewShortcutBinding } from '@/lib/previewTypes'
import { WorkbenchKeyboardBridge } from '@/workbench/WorkbenchKeyboardBridge'
import { navigateWorkbenchTabs } from '@/lib/workbenchTabNavigation'

vi.mock('@/lib/platform', () => ({ isMacPlatform: () => true }))
vi.mock('@/lib/workbenchTabNavigation', () => ({ navigateWorkbenchTabs: vi.fn(async () => {}) }))
vi.mock('@/terminal/openNewTerminalTab', () => ({ openNewTerminalTab: vi.fn(async () => {}) }))

interface Interaction {
  poll(bindings: PreviewShortcutBinding[]): { commands: string[] }
}
const interaction = () => (window as unknown as { __yuzoraBrowser: Interaction }).__yuzoraBrowser

beforeEach(() => {
  vi.clearAllMocks()
  useKeyboardSettingsStore.setState({ overrides: {} })
  window.eval(script)
  interaction().poll([])
  document.body.replaceChildren()
  document.body.removeAttribute('data-shortcut-capture')
})
afterEach(cleanup)

function surface(kind: 'top' | 'same-origin' | 'child') {
  const target = document.body
  let bindings: PreviewShortcutBinding[] = []
  if (kind !== 'top') {
    const childDocument = kind === 'same-origin'
      ? document.body.appendChild(document.createElement('iframe')).contentDocument!
      : document.implementation.createHTMLDocument('child')
    const top = { postMessage: vi.fn() }
    let message!: (event: { source: unknown; data: unknown }) => void
    // The script also listens for pageshow; only the message listener carries bindings.
    const child = { top, addEventListener: (type: string, listener: typeof message) => { if (type === 'message') message = listener } }
    new Function('window', 'document', script)(child, childDocument)
    return {
      target: childDocument.body,
      setBindings: (next: PreviewShortcutBinding[]) => message({ source: top, data: { type: 'yuzora-tab-bindings', bindings: next } }),
      commands: () => {
        const commands = top.postMessage.mock.calls.map(([data]) => data).filter(data => data.type === 'yuzora-tab-command').map(data => data.id)
        top.postMessage.mockClear()
        return commands
      },
    }
  }
  return {
    target,
    setBindings: (next: PreviewShortcutBinding[]) => { bindings = next; interaction().poll(bindings) },
    commands: () => interaction().poll(bindings).commands,
  }
}

interface KeyCase {
  name: string
  binding: string
  keys: KeyboardEventInit
  matches: boolean
  altGraph?: boolean
  capture?: boolean
}

const cases: KeyCase[] = [
  { name: 'shift semicolon', binding: 'Ctrl+Shift+;', keys: { key: ':', code: 'Semicolon', ctrlKey: true, shiftKey: true }, matches: true },
  { name: 'option letter', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true }, matches: true },
  { name: 'option digit', binding: 'Alt+0', keys: { key: 'º', code: 'Digit0', altKey: true }, matches: true },
  { name: 'dead backquote', binding: 'Alt+Shift+`', keys: { key: 'Dead', code: 'Backquote', altKey: true, shiftKey: true }, matches: true },
  ...([
    [',', '<', 'Comma'], ['.', '>', 'Period'], ['/', '?', 'Slash'],
    ['[', '{', 'BracketLeft'], [']', '}', 'BracketRight'], ['\\', '|', 'Backslash'], ['-', '_', 'Minus'],
  ] as const).map(([key, shifted, code]) => ({ name: `shift ${code}`, binding: `Ctrl+Shift+${key}`, keys: { key: shifted, code, ctrlKey: true, shiftKey: true }, matches: true })),
  { name: 'default tab', binding: 'Ctrl+Tab', keys: { key: 'Tab', code: 'Tab', ctrlKey: true }, matches: true },
  { name: 'function key', binding: 'Alt+F2', keys: { key: 'F2', code: 'F2', altKey: true }, matches: true },
  { name: 'direct letter match', binding: 'Alt+J', keys: { key: 'j', code: 'KeyJ', altKey: true }, matches: true },
  { name: 'no physical fallback without Alt or Shift', binding: 'Ctrl+;', keys: { key: ':', code: 'Semicolon', ctrlKey: true }, matches: false },
  { name: 'wrong physical key', binding: 'Alt+J', keys: { key: '∆', code: 'KeyK', altKey: true }, matches: false },
  { name: 'extra modifier', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true, ctrlKey: true }, matches: false },
  { name: 'missing modifier', binding: 'Ctrl+Shift+;', keys: { key: ':', code: 'Semicolon', shiftKey: true }, matches: false },
  { name: 'composition', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true, isComposing: true }, matches: false },
  { name: 'repeat', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true, repeat: true }, matches: false },
  { name: 'AltGraph', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true }, altGraph: true, matches: false },
  { name: 'capturing input', binding: 'Alt+J', keys: { key: '∆', code: 'KeyJ', altKey: true }, capture: true, matches: false },
  { name: 'unsupported physical function fallback', binding: 'Alt+F2', keys: { key: 'Dead', code: 'F2', altKey: true }, matches: false },
]

function keyEvent(spec: KeyCase) {
  const event = new KeyboardEvent('keydown', { ...spec.keys, bubbles: true, cancelable: true })
  if (spec.altGraph) Object.defineProperty(event, 'getModifierState', { value: (key: string) => key === 'AltGraph' })
  return event
}

describe.each(['top', 'same-origin', 'child'] as const)('Browser shortcut parity in %s', kind => {
  it.each(cases)('$name', spec => {
    expect(useKeyboardSettingsStore.getState().setBinding('nextTab', spec.binding)).toBe(true)
    const mainTarget = document.createElement('div'), run = vi.fn()
    if (spec.capture) mainTarget.dataset.shortcutCapture = ''
    mainTarget.addEventListener('keydown', event => { dispatchAppShortcut(event, 'nextTab', run) })
    const mainEvent = keyEvent(spec)
    mainTarget.dispatchEvent(mainEvent)
    expect(run).toHaveBeenCalledTimes(spec.matches ? 1 : 0)
    expect(mainEvent.defaultPrevented).toBe(spec.matches)

    const view = surface(kind)
    view.setBindings(tabShortcutBindings())
    if (spec.capture) view.target.dataset.shortcutCapture = ''
    const event = keyEvent(spec)
    view.target.dispatchEvent(event)
    expect(view.commands()).toEqual(spec.matches ? ['nextTab'] : [])
    expect(event.defaultPrevented).toBe(spec.matches)
  })

  it('uses changed and reset settings without retaining old physical bindings', () => {
    const view = surface(kind)
    expect(useKeyboardSettingsStore.getState().setBinding('nextTab', 'Ctrl+Shift+;')).toBe(true)
    view.setBindings(tabShortcutBindings())
    view.target.dispatchEvent(keyEvent(cases[0]))
    expect(view.commands()).toEqual(['nextTab'])
    useKeyboardSettingsStore.getState().reset()
    view.setBindings(tabShortcutBindings())
    view.target.dispatchEvent(keyEvent(cases[0]))
    expect(view.commands()).toEqual([])
    view.target.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', code: 'Tab', ctrlKey: true, bubbles: true, cancelable: true }))
    expect(view.commands()).toEqual(['nextTab'])
    view.setBindings([])
    view.target.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', ctrlKey: true, bubbles: true, cancelable: true }))
    expect(view.commands()).toEqual([])
  })

  it.each([
    { first: 'nextTab', firstBinding: 'Alt+A', second: 'tab1', secondBinding: 'Alt+Q', command: 'nextTab', navigation: { direction: 1 } },
    { first: 'nextTab', firstBinding: 'Alt+Q', second: 'tab1', secondBinding: 'Alt+A', command: 'nextTab', navigation: { direction: 1 } },
    { first: 'previousTab', firstBinding: 'Alt+A', second: 'tab1', secondBinding: 'Alt+Q', command: 'previousTab', navigation: { direction: -1 } },
    { first: 'tab1', firstBinding: 'Alt+Q', second: 'tab2', secondBinding: 'Alt+A', command: 'tab1', navigation: { index: 0 } },
    { first: 'nextTab', firstBinding: 'Alt+A', second: 'previousTab', secondBinding: 'Alt+Q', command: 'nextTab', navigation: { direction: 1 } },
    { first: 'nextTab', firstBinding: 'Alt+Q', second: 'previousTab', secondBinding: 'Alt+A', command: 'nextTab', navigation: { direction: 1 } },
  ] as const)('matches main dispatch priority for $firstBinding/$secondBinding on $first/$second', spec => {
    expect(useKeyboardSettingsStore.getState().setBinding(spec.first, spec.firstBinding)).toBe(true)
    expect(useKeyboardSettingsStore.getState().setBinding(spec.second, spec.secondBinding)).toBe(true)
    const keys = { key: 'a', code: 'KeyQ', altKey: true }
    const main = render(createElement(WorkbenchKeyboardBridge))
    fireEvent.keyDown(window, keys)
    expect(navigateWorkbenchTabs).toHaveBeenCalledExactlyOnceWith(spec.navigation)
    main.unmount()
    const view = surface(kind)
    view.setBindings(tabShortcutBindings())
    const event = new KeyboardEvent('keydown', { ...keys, bubbles: true, cancelable: true })
    view.target.dispatchEvent(event)
    expect(view.commands()).toEqual([spec.command])
    expect(event.defaultPrevented).toBe(true)
  })
})
