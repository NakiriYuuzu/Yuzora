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

// What rpc.ts does on a real reconnect: the socket closes ("waiting"), comes back "ready" before the hello answer (the epoch is still the old one), and only when hello lands does the epoch bump.
const dropSocket = () => act(async () => useCompanion.setState({ socket: "waiting" }))
const socketReady = () => act(async () => useCompanion.setState({ socket: "ready" }))
const helloAnswered = () => act(async () => useCompanion.setState({ epoch: 2 }))

it("shows no previous result from the moment the socket drops until the reconnect's result lands, when the caller asked not to keep it", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, false))
  await act(async () => calls[0].resolve("first"))
  expect(result.current.data).toBe("first")
  await dropSocket()
  expect(result.current.data).toBeNull()
  await socketReady()
  await waitFor(() => expect(calls).toHaveLength(2))
  expect(result.current).toMatchObject({ data: null, error: null })
  await helloAnswered()
  await waitFor(() => expect(calls).toHaveLength(3))
  expect(result.current).toMatchObject({ data: null, error: null })
  // The load the epoch bump replaced answers late and is ignored.
  await act(async () => calls[1].resolve("replaced"))
  expect(result.current.data).toBeNull()
  await act(async () => calls[2].resolve("second"))
  expect(result.current).toMatchObject({ data: "second", error: null })
})

it("shows the result of the load that the socket's return started as soon as it lands, before hello", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, false))
  await act(async () => calls[0].resolve("first"))
  await dropSocket()
  await socketReady()
  await waitFor(() => expect(calls).toHaveLength(2))
  await act(async () => calls[1].resolve("second"))
  expect(result.current).toMatchObject({ data: "second", error: null })
})

it("keeps the previous result across the same reconnect by default, with one load per trigger", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load))
  await act(async () => calls[0].resolve("first"))
  await dropSocket()
  expect(calls).toHaveLength(1)
  expect(result.current.data).toBe("first")
  await socketReady()
  await waitFor(() => expect(calls).toHaveLength(2))
  expect(result.current.data).toBe("first")
  await helloAnswered()
  await waitFor(() => expect(calls).toHaveLength(3))
  expect(result.current.data).toBe("first")
  await act(async () => calls[2].resolve("second"))
  expect(result.current.data).toBe("second")
})

it("leaves nothing but the error when the load the socket's return started fails before hello, and loads again at hello", async () => {
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, false))
  await act(async () => calls[0].resolve("first"))
  await dropSocket()
  await socketReady()
  await waitFor(() => expect(calls).toHaveLength(2))
  await act(async () => calls[1].reject(new RpcError("git-failed")))
  expect(result.current).toMatchObject({ data: null, error: "git-failed" })
  await helloAnswered()
  await waitFor(() => expect(calls).toHaveLength(3))
  expect(result.current).toMatchObject({ data: null, error: "git-failed" })
  await act(async () => calls[2].resolve("second"))
  expect(result.current).toMatchObject({ data: "second", error: null })
})

it.each([[true], [false]])("loads once when the socket becomes ready after the hook mounted (keepPrevious %s)", async (keepPrevious) => {
  useCompanion.setState({ socket: "waiting" })
  const { calls, load } = held()
  const { result } = renderHook(() => useLoad(load, 0, keepPrevious))
  expect(calls).toHaveLength(0)
  await socketReady()
  await waitFor(() => expect(calls).toHaveLength(1))
  expect(result.current.data).toBeNull()
  await act(async () => calls[0].resolve("first"))
  expect(result.current.data).toBe("first")
  expect(calls).toHaveLength(1)
})
