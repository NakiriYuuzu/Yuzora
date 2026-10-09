import { describe, expect, it } from "vitest"

import { queueHerdrMutation } from "./herdrMutationQueue"

function deferred() {
  let resolve: () => void = () => undefined
  let reject: (error: Error) => void = () => undefined
  const promise = new Promise<void>((done, fail) => { resolve = done; reject = fail })
  return { promise, resolve, reject }
}

describe("queueHerdrMutation", () => {
  it("starts a session's next mutation only after the previous one settles", async () => {
    const order: string[] = []
    const first = deferred()
    const a = queueHerdrMutation("work", async () => { order.push("a:start"); await first.promise; order.push("a:end") })
    const b = queueHerdrMutation("work", async () => { order.push("b") })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(order).toEqual(["a:start"])
    first.resolve()
    await Promise.all([a, b])
    expect(order).toEqual(["a:start", "a:end", "b"])
  })

  it("keeps going after a failed mutation and reports the failure to its caller", async () => {
    const first = deferred()
    const a = queueHerdrMutation("work", () => first.promise)
    const b = queueHerdrMutation("work", async () => "done")
    first.reject(new Error("swap failed"))
    await expect(a).rejects.toThrow("swap failed")
    await expect(b).resolves.toBe("done")
  })

  it("does not hold one session's mutations behind another's", async () => {
    const blocked = deferred()
    void queueHerdrMutation("work", () => blocked.promise)
    await expect(queueHerdrMutation("other", async () => "ran")).resolves.toBe("ran")
    blocked.resolve()
  })
})
