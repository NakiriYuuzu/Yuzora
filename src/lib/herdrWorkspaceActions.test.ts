import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { commandFor } from '@/app/workbench/contextMenuDefs'
import type { ContextMenuRequestFor } from '@/app/workbench/contextMenuModel'
import { herdrInitialState, useHerdrStore } from '@/state/herdrStore'
import { herdrWorkspaceMove, herdrWorkspaceMoveBlock } from './herdrIpc'
import type { HerdrSessionRuntime, HerdrSpaceInfo } from './herdrTypes'
import i18n from './i18n'

vi.mock('./herdrIpc', async () => ({
  ...await vi.importActual<typeof import('./herdrIpc')>('./herdrIpc'),
  herdrWorkspaceMove: vi.fn(),
  herdrWorkspaceMoveBlock: vi.fn(),
}))

const sessionName = '["wsl:Ubuntu-26.04","test"]'
const space = (id: string, key?: string, linked = false): HerdrSpaceInfo => ({ id, label: id, focused: false, order: 0, worktreeGroupKey: key, isLinkedWorktree: linked })
const refreshSnapshot = vi.fn(async () => true)
const originalRefresh = useHerdrStore.getState().refreshSnapshot
function fixture(spaces = [space('a'), space('b'), space('c')], block = true) {
  useHerdrStore.setState({ ...herdrInitialState, refreshSnapshot, runtimesBySession: {
    [sessionName]: {
      connectionState: 'ready', errorMessage: null,
      capabilities: { server: { running: true, compatible: true }, api: { workspaceMove: true, workspaceMoveBlock: block } },
      snapshot: { spaces },
    } as HerdrSessionRuntime,
  } })
}
function action(id: string, direction: 'up' | 'down') {
  const request: ContextMenuRequestFor<'herdrSpace'> = { kind: 'herdrSpace', sessionName, workspaceId: id }
  const command = commandFor(request, direction === 'up' ? 'cmHerdrMoveSpaceUp' : 'cmHerdrMoveSpaceDown')!
  return { request, command, run: () => command.executor(request), availability: () => command.availability(request) }
}
const order = () => useHerdrStore.getState().runtimesBySession[sessionName].snapshot!.spaces.map(s => s.id)

beforeEach(() => {
  vi.clearAllMocks()
  fixture()
  vi.mocked(herdrWorkspaceMove).mockResolvedValue({ workspaceIds: ['b', 'a', 'c'] })
  vi.mocked(herdrWorkspaceMoveBlock).mockResolvedValue({ workspaceIds: ['b', 'a', 'c'] })
})
afterEach(() => useHerdrStore.setState({ ...herdrInitialState, refreshSnapshot: originalRefresh }))

describe('Space move menu', () => {
  it('localizes both commands and disables only the boundary direction', async () => {
    const up = action('a', 'up'), down = action('a', 'down')
    await i18n.changeLanguage('en')
    expect(up.command.label(up.request)).toBe('Move Up')
    expect(down.command.label(down.request)).toBe('Move Down')
    await i18n.changeLanguage('zh-TW')
    expect(up.command.label(up.request)).toBe('上移')
    expect(down.command.label(down.request)).toBe('下移')
    expect(up.availability()).toMatchObject({ enabled: false, disabledReasonKey: 'contextMenu.disabled.herdrFirstSpace' })
    expect(down.availability().enabled).toBe(true)
    expect(action('c', 'down').availability()).toMatchObject({ enabled: false, disabledReasonKey: 'contextMenu.disabled.herdrLastSpace' })
    expect(action('c', 'up').availability().enabled).toBe(true)
    await expect(up.run()).resolves.toBe('cancelled')
    expect(herdrWorkspaceMoveBlock).not.toHaveBeenCalled()
  })

  it.each([['a', 'down', 'c'], ['b', 'up', 'a']] as const)('moves %s %s with one block request scoped to its session', async (id, direction, beforeWorkspaceId) => {
    await expect(action(id, direction).run()).resolves.toBe('completed')
    expect(herdrWorkspaceMoveBlock).toHaveBeenCalledExactlyOnceWith({ sessionName, workspaceIds: [id], beforeWorkspaceId })
    expect(order()).toEqual(['b', 'a', 'c'])
    expect(refreshSnapshot).toHaveBeenCalledExactlyOnceWith(sessionName)
  })

  it('uses the original insertion boundary for a downward legacy move', async () => {
    fixture(undefined, false)
    await action('a', 'down').run()
    expect(herdrWorkspaceMove).toHaveBeenCalledExactlyOnceWith({ sessionName, workspaceId: 'a', insertIndex: 2 })
    expect(herdrWorkspaceMoveBlock).not.toHaveBeenCalled()
  })

  it('moves the entire group past the neighboring group and refuses independent children', async () => {
    fixture([space('a', 'repo-a'), space('ac', 'repo-a', true), space('b', 'repo-b'), space('bc', 'repo-b', true), space('c')])
    vi.mocked(herdrWorkspaceMoveBlock).mockResolvedValueOnce({ workspaceIds: ['b', 'bc', 'a', 'ac', 'c'] })
    expect(action('ac', 'down').availability()).toMatchObject({ enabled: false, disabledReasonKey: 'contextMenu.disabled.herdrLinkedWorktree' })
    await action('a', 'down').run()
    expect(herdrWorkspaceMoveBlock).toHaveBeenCalledExactlyOnceWith({ sessionName, workspaceIds: ['a', 'ac'], beforeWorkspaceId: 'c' })
    expect(order()).toEqual(['b', 'bc', 'a', 'ac', 'c'])
  })

  it('does not skip an incompletely identified neighbor or split an unsupported group', () => {
    fixture([space('a'), { ...space('b'), repoKey: 'repo-b' }, { ...space('bc', undefined, true), repoKey: 'repo-b' }, space('c')])
    expect(action('a', 'down').availability().enabled).toBe(false)
    expect(action('b', 'down').availability().disabledReasonKey).toBe('contextMenu.disabled.herdrGroupIncomplete')
    fixture([space('a', 'r'), space('ac', 'r', true), space('b')], false)
    expect(action('a', 'down').availability().enabled).toBe(false)
  })

  it('recomputes the neighbor if the order changes while the menu stays open', async () => {
    const move = action('a', 'down')
    fixture([space('c'), space('a'), space('b')])
    await move.run()
    expect(herdrWorkspaceMoveBlock).toHaveBeenCalledExactlyOnceWith({ sessionName, workspaceIds: ['a'], beforeWorkspaceId: null })
  })

  it('rejects another menu move until the first request and reconciliation finish', async () => {
    let finish!: (value: { workspaceIds: string[] }) => void
    vi.mocked(herdrWorkspaceMoveBlock).mockReturnValueOnce(new Promise(resolve => { finish = resolve }))
    const first = action('a', 'down').run()
    await expect(action('c', 'up').run()).resolves.toBe('cancelled')
    finish({ workspaceIds: ['b', 'a', 'c'] })
    await first
    expect(herdrWorkspaceMoveBlock).toHaveBeenCalledOnce()
    expect(action('c', 'up').availability().enabled).toBe(true)
  })

  it.each(['host-request-limit', 'timeout', 'permission denied'])('reconciles %s without issuing another mutation', async message => {
    vi.mocked(herdrWorkspaceMoveBlock).mockRejectedValueOnce(new Error(message))
    await expect(action('a', 'down').run()).rejects.toThrow(message)
    expect(herdrWorkspaceMove).not.toHaveBeenCalled()
    expect(refreshSnapshot).toHaveBeenCalledOnce()
    expect(action('c', 'up').availability().enabled).toBe(true)
  })
})
