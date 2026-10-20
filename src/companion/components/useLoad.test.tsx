import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, expect, it } from "vitest"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"
import { useLoad } from "./useLoad"

beforeEach(() => useCompanion.setState({ socket: "ready", epoch: 1 }))

/** A `load` that answers each call through the promise the test holds. */
function held() {
  const calls: { resolve: (value: string) => void; reject: (error: unknown) => void }[] = []
  const load = () => new Promise<string>((resolve, reject) => calls.push({ resolve, reject }))
  return { calls, load }
}

it.each([
  ["a retry", (reload: () => void) => reload()],
  ["a reconnect", () => useCompanion.setState({ epoch: 2 })],
])("keeps the previous result while %s is in flight by default", async (_name, trigger) => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load))
  await act(async () => calls[0].resolve("first"))
  expect(result.current.data).toBe("first")
  await act(async () => trigger(result.current.reload))
  await waitFor(() => expect(calls).toHaveLength(2))
  expect(result.current.data).toBe("first")
  await act(async () => calls[1].resolve("second"))
  expect(result.current.data).toBe("second")
})

it("keeps the previous result next to the error of a failed reload by default", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load))
  await act(async () => calls[0].resolve("first"))
  await act(async () => result.current.reload())
  await waitFor(() => expect(calls).toHaveLength(2))
  await act(async () => calls[1].reject(new RpcError("git-failed")))
  expect(result.current).toMatchObject({ data: "first", error: "git-failed" })
})

it.each([
  ["a retry", (reload: () => void) => reload()],
  ["a reconnect", () => useCompanion.setState({ epoch: 2 })],
])("drops the previous result when %s starts if the caller asked not to keep it", async (_name, trigger) => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, false))
  await act(async () => calls[0].resolve("first"))
  expect(result.current.data).toBe("first")
  await act(async () => trigger(result.current.reload))
  await waitFor(() => expect(calls).toHaveLength(2))
  expect(result.current).toMatchObject({ data: null, error: null })
  await act(async () => calls[1].resolve("second"))
  expect(result.current.data).toBe("second")
})

it("leaves nothing but the error when a reload fails after the previous result was dropped", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, false))
  await act(async () => calls[0].resolve("first"))
  await act(async () => result.current.reload())
  await waitFor(() => expect(calls).toHaveLength(2))
  await act(async () => calls[1].reject(new RpcError("git-failed")))
  expect(result.current).toMatchObject({ data: null, error: "git-failed" })
})

it.each([[true], [false]])("keeps the error of a failed load while a reconnect's load is in flight (keepPrevious %s)", async (keepPrevious) => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, keepPrevious))
  await act(async () => calls[0].reject(new RpcError("git-failed")))
  expect(result.current.error).toBe("git-failed")
  await act(async () => useCompanion.setState({ epoch: 2 }))
  await waitFor(() => expect(calls).toHaveLength(2))
  expect(result.current.error).toBe("git-failed")
  await act(async () => calls[1].resolve("second"))
  expect(result.current).toMatchObject({ data: "second", error: null })
})
