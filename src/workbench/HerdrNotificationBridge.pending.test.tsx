import { act, cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { HerdrNotificationBridge } from './HerdrNotificationBridge'
import { herdrInitialState, useHerdrStore } from '@/state/herdrStore'
import { useHerdrNotificationStore } from '@/state/herdrNotificationStore'
import { useWorkspaceStore } from '@/state/workspaceStore'
import { useHerdrNativeStore } from '@/state/herdrNativeStore'
import type { HerdrAgentStatus, HerdrSnapshot } from '@/lib/herdrTypes'

const mocks = vi.hoisted(() => ({ permission: vi.fn(), system: vi.fn(), toast: vi.fn() }))
vi.mock('sonner', () => ({ toast: mocks.toast }))
vi.mock('@tauri-apps/plugin-notification', () => ({ isPermissionGranted: mocks.permission, sendNotification: mocks.system }))

function deferred() {
  let resolve!: (value: boolean) => void
  let reject!: (error: Error) => void
  const promise = new Promise<boolean>((done, fail) => { resolve = done; reject = fail })
  return { promise, resolve, reject }
}

function snapshot(sessionName: string, status: HerdrAgentStatus = 'working'): HerdrSnapshot {
  return {
    herdrSessionId: sessionName, protocol: 22, version: '0.9.1', raw: {},
    spaces: [
      { id: 'w1', label: 'One', order: 0, focused: true },
      { id: 'w2', label: 'Two', order: 1, focused: false },
    ], tabs: [], terminals: [], focusedPaneId: 'p1',
    agents: [{ id: 'a1', name: 'Agent', paneId: 'p1', tabId: 't1', workspaceId: 'w1', status }],
  }
}

function connect(sessionName: string) {
  useHerdrStore.getState().applySnapshot(sessionName, snapshot(sessionName))
  useHerdrStore.setState(state => ({ runtimesBySession: {
    ...state.runtimesBySession,
    [sessionName]: { ...state.runtimesBySession[sessionName], connectionState: 'ready' },
  } }))
  useHerdrStore.getState().setEventsHealth(sessionName, true, `live-${sessionName}`)
  expect(useHerdrStore.getState().runtimesBySession[sessionName].connectionState).toBe('ready')
}

async function live(status: string, sessionName = 'default') {
  await act(async () => {
    useHerdrStore.getState().applySubscriptionEvent(sessionName, {
      type: 'agent_status_changed', subscriptionId: `live-${sessionName}`,
      paneId: 'p1', workspaceId: 'w1', agentStatus: status, stateLabels: {},
    })
  })
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.permission.mockResolvedValue(false)
  vi.spyOn(document, 'hasFocus').mockReturnValue(false)
  vi.spyOn(Date, 'now').mockReturnValue(100)
  useHerdrStore.setState({ ...herdrInitialState, attentionByKey: new Map(), runtimesBySession: {} })
  useHerdrNotificationStore.setState({ toast: false, system: true, sound: false, done: true, blocked: true })
  useWorkspaceStore.setState({ groups: [], activeGroupIndex: 0 })
  useHerdrNativeStore.setState({ selection: null })
  connect('default')
})
afterEach(() => { cleanup(); vi.restoreAllMocks() })

it.each(['done', 'blocked'])('delivers one pending %s notification after a duplicate live event', async kind => {
  const waiting = deferred()
  mocks.permission.mockReturnValue(waiting.promise)
  render(<HerdrNotificationBridge />)
  await live(kind)
  vi.mocked(Date.now).mockReturnValue(200)
  await live(kind)
  expect(useHerdrStore.getState().attentionItems('default')[0]).toMatchObject({ kind, updatedAt: 200 })
  expect(mocks.permission).toHaveBeenCalledOnce()
  await act(async () => { waiting.resolve(true) })
  expect(mocks.system).toHaveBeenCalledOnce()
})

it.each([
  ['done', false], ['blocked', false], ['done', true], ['blocked', true],
] as const)('keeps distinct %s turns separate at the same clock timestamp (old first: %s)', async (kind, oldFirst) => {
  const old = deferred(), current = deferred()
  mocks.permission.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise)
  render(<HerdrNotificationBridge />)
  await live(kind)
  await live('working')
  await live(kind)
  expect(mocks.permission).toHaveBeenCalledTimes(2)
  if (oldFirst) {
    await act(async () => { old.resolve(true) })
    expect(mocks.system).not.toHaveBeenCalled()
    await act(async () => { current.resolve(true) })
  } else {
    await act(async () => { current.resolve(true) })
    expect(mocks.system).toHaveBeenCalledOnce()
    await act(async () => { old.resolve(true) })
  }
  expect(mocks.system).toHaveBeenCalledOnce()
})

it('does not let an old failed request remove the pending current turn', async () => {
  const old = deferred(), current = deferred()
  const warning = vi.spyOn(console, 'warn').mockImplementation(() => {})
  mocks.permission.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise)
  render(<HerdrNotificationBridge />)
  await live('done'); await live('working'); await live('done')
  await act(async () => { old.reject(new Error('old request failed')) })
  expect(warning).toHaveBeenCalledOnce()
  expect(mocks.system).not.toHaveBeenCalled()
  await act(async () => { current.resolve(true) })
  expect(mocks.system).toHaveBeenCalledOnce()
})

it('keeps identical pane IDs in different Sessions independent', async () => {
  connect('other')
  const first = deferred(), second = deferred()
  mocks.permission.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
  render(<HerdrNotificationBridge />)
  await live('done')
  await live('blocked', 'other')
  vi.mocked(Date.now).mockReturnValue(200)
  await live('done')
  expect(mocks.permission).toHaveBeenCalledTimes(2)
  await act(async () => { second.resolve(true); first.resolve(true) })
  expect(mocks.system).toHaveBeenCalledTimes(2)
  expect(mocks.system.mock.calls.map(([notification]) => notification.body)).toEqual(['other · w1', 'default · w1'])
})

it.each(['seen', 'cleared', 'snapshot-cleared', 'unmounted'])('drops pending delivery when the live transition is %s', async change => {
  const waiting = deferred()
  mocks.permission.mockReturnValue(waiting.promise)
  const view = render(<HerdrNotificationBridge />)
  await live('done')
  expect(mocks.permission).toHaveBeenCalledOnce()
  await act(async () => {
    if (change === 'seen') useHerdrStore.getState().markAttentionSeen('default', 'p1')
    else if (change === 'cleared') useHerdrStore.setState({ attentionByKey: new Map() })
    else if (change === 'snapshot-cleared') useHerdrStore.getState().applySnapshot('default', snapshot('default', 'working'))
    else view.unmount()
    waiting.resolve(true)
  })
  expect(mocks.system).not.toHaveBeenCalled()
  expect(mocks.permission).toHaveBeenCalledOnce()
})

it.each(['refresh', 'reorder', 'recovery-same-state'])('keeps a known live notification during %s when its unread attention survives', async change => {
  const waiting = deferred()
  mocks.permission.mockReturnValue(waiting.promise)
  render(<HerdrNotificationBridge />)
  await live('done')
  const before = useHerdrStore.getState()
  await act(async () => {
    if (change === 'reorder') expect(useHerdrStore.getState().applyWorkspaceOrder('default', ['w2', 'w1'])).toBe(true)
    else {
      if (change === 'recovery-same-state') useHerdrStore.getState().setEventsHealth('default', false)
      useHerdrStore.getState().applySnapshot('default', snapshot('default', 'done'))
    }
  })
  expect(useHerdrStore.getState().attentionByKey).toBe(before.attentionByKey)
  expect(useHerdrStore.getState().runtimesBySession.default.baseSnapshot).not.toBe(before.runtimesBySession.default.baseSnapshot)
  expect(mocks.permission).toHaveBeenCalledOnce()
  await act(async () => { waiting.resolve(true) })
  expect(mocks.system).toHaveBeenCalledOnce()
})

it('releases its subscription across 100 pending-notification mount/close cycles', async () => {
  const subscribe = useHerdrStore.subscribe
  let subscriptions = 0
  vi.spyOn(useHerdrStore, 'subscribe').mockImplementation(listener => {
    subscriptions++
    const unsubscribe = subscribe(listener)
    return () => { subscriptions--; unsubscribe() }
  })
  for (let cycle = 0; cycle < 110; cycle++) {
    await live('working')
    const waiting = deferred()
    mocks.permission.mockReturnValueOnce(waiting.promise)
    const view = render(<HerdrNotificationBridge />)
    expect(subscriptions).toBe(1)
    await live('done')
    expect(mocks.permission).toHaveBeenCalledTimes(cycle + 1)
    view.unmount()
    expect(subscriptions).toBe(0)
    await act(async () => { waiting.resolve(true) })
    expect(mocks.system).not.toHaveBeenCalled()
  }
})
