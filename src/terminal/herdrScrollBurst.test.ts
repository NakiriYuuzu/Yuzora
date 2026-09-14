import { expect, it, vi } from 'vitest'
import { createPaneScrollController } from './herdrScrollController'

it('bounds 200 unresolved gestures to one RPC and keeps the latest visible intent', async () => {
  const change = vi.fn()
  const write = vi.fn(() => new Promise<null>(() => {}))
  const controller = createPaneScrollController({ read: async () => ({ offsetFromBottom: 0, maxOffsetFromBottom: 2000, viewportRows: 24 }), write, change, allowed: () => true })
  await controller.refresh()
  for (let i = 1; i <= 200; i++) controller.move(i)
  expect(change).toHaveBeenLastCalledWith({ offsetFromBottom: 200, maxOffsetFromBottom: 2000, viewportRows: 24 })
  expect(write).toHaveBeenCalledTimes(1)
  controller.dispose()
})
