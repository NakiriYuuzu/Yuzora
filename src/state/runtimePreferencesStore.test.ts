import { afterEach, beforeEach, expect, it, vi } from "vitest"

const ipc = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn() }))
vi.mock("@/lib/runtimePreferencesIpc", () => ({ runtimePreferencesGet: ipc.get, runtimePreferencesSet: ipc.set }))
import { LEGACY_RUNTIME_PREFERENCES_KEY, useRuntimePreferencesStore } from "./runtimePreferencesStore"

const initial = useRuntimePreferencesStore.getState()
beforeEach(() => {
  vi.resetAllMocks()
  const values = new Map<string, string>()
  vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value), removeItem: (key: string) => values.delete(key) })
  vi.spyOn(console, "warn").mockImplementation(() => undefined)
  useRuntimePreferencesStore.setState({ ...initial, wslEnabled: false, hydrated: false }, true)
})
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks() })

const legacy = () => window.localStorage.getItem(LEGACY_RUNTIME_PREFERENCES_KEY)

it("starts disabled and unhydrated", () => {
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: false, hydrated: false })
})

it("hydrates from the backend", async () => {
  ipc.get.mockResolvedValue({ wslEnabled: true })
  await useRuntimePreferencesStore.getState().hydrate()
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: true, hydrated: true })
  expect(ipc.set).not.toHaveBeenCalled()
})

it("migrates the legacy localStorage flag once and clears it after the backend accepts", async () => {
  window.localStorage.setItem(LEGACY_RUNTIME_PREFERENCES_KEY, JSON.stringify({ wslEnabled: true }))
  ipc.get.mockResolvedValue({ wslEnabled: false })
  ipc.set.mockResolvedValue({ wslEnabled: true })
  await useRuntimePreferencesStore.getState().hydrate()
  expect(ipc.set).toHaveBeenCalledWith(true)
  expect(useRuntimePreferencesStore.getState().wslEnabled).toBe(true)
  expect(legacy()).toBeNull()
})

it("keeps the legacy flag when the migration write fails so the next launch retries", async () => {
  window.localStorage.setItem(LEGACY_RUNTIME_PREFERENCES_KEY, JSON.stringify({ wslEnabled: true }))
  ipc.get.mockResolvedValue({ wslEnabled: false })
  ipc.set.mockRejectedValue(new Error("disk full"))
  await useRuntimePreferencesStore.getState().hydrate()
  expect(legacy()).not.toBeNull()
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: false, hydrated: true })
})

it("treats a missing backend command as disabled without blocking and keeps the legacy flag", async () => {
  window.localStorage.setItem(LEGACY_RUNTIME_PREFERENCES_KEY, JSON.stringify({ wslEnabled: true }))
  ipc.get.mockRejectedValue(new Error("command runtime_preferences_get not found"))
  await useRuntimePreferencesStore.getState().hydrate()
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: false, hydrated: true })
  expect(legacy()).not.toBeNull()
})

it("does not migrate when the backend already holds true", async () => {
  window.localStorage.setItem(LEGACY_RUNTIME_PREFERENCES_KEY, JSON.stringify({ wslEnabled: true }))
  ipc.get.mockResolvedValue({ wslEnabled: true })
  await useRuntimePreferencesStore.getState().hydrate()
  expect(ipc.set).not.toHaveBeenCalled()
})

it("updates state only after the backend confirms", async () => {
  ipc.set.mockResolvedValue({ wslEnabled: true })
  await useRuntimePreferencesStore.getState().setWslEnabled(true)
  expect(ipc.set).toHaveBeenCalledWith(true)
  expect(useRuntimePreferencesStore.getState().wslEnabled).toBe(true)
})

it("keeps the previous value and rejects when the backend refuses", async () => {
  useRuntimePreferencesStore.setState({ wslEnabled: true })
  ipc.set.mockRejectedValue(new Error("write failed"))
  await expect(useRuntimePreferencesStore.getState().setWslEnabled(false)).rejects.toThrow("write failed")
  expect(useRuntimePreferencesStore.getState().wslEnabled).toBe(true)
})

it("does not let an older in-flight hydrate overwrite a value the user saved meanwhile", async () => {
  let resolveGet!: (value: { wslEnabled: boolean }) => void
  ipc.get.mockReturnValue(new Promise(resolve => { resolveGet = resolve }))
  ipc.set.mockResolvedValue({ wslEnabled: true })
  const hydrating = useRuntimePreferencesStore.getState().hydrate()
  await useRuntimePreferencesStore.getState().setWslEnabled(true)
  resolveGet({ wslEnabled: false })
  await hydrating
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: true, hydrated: true })
})

it("does not migrate the legacy flag when the user saved a value while the backend read was in flight", async () => {
  window.localStorage.setItem(LEGACY_RUNTIME_PREFERENCES_KEY, JSON.stringify({ wslEnabled: true }))
  let resolveGet!: (value: { wslEnabled: boolean }) => void
  ipc.get.mockReturnValue(new Promise(resolve => { resolveGet = resolve }))
  ipc.set.mockResolvedValue({ wslEnabled: false })
  const hydrating = useRuntimePreferencesStore.getState().hydrate()
  await useRuntimePreferencesStore.getState().setWslEnabled(false)
  ipc.set.mockClear()
  resolveGet({ wslEnabled: false })
  await hydrating
  expect(ipc.set).not.toHaveBeenCalled()
  expect(useRuntimePreferencesStore.getState()).toMatchObject({ wslEnabled: false, hydrated: true })
})
