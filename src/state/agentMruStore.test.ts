import { beforeEach, expect, it } from "vitest"
import { AGENT_MRU_LIMIT, agentMruKey, orderAgentsByRecency, useAgentMruStore } from "./agentMruStore"

beforeEach(() => useAgentMruStore.setState({ keys: [] }))

it("keeps the most recent Agent first without duplicates and respects the limit", () => {
  const { touch } = useAgentMruStore.getState()
  touch("s", "a"); touch("s", "b"); touch("s", "a")
  expect(useAgentMruStore.getState().keys).toEqual([agentMruKey("s", "a"), agentMruKey("s", "b")])
  for (let i = 0; i < AGENT_MRU_LIMIT + 5; i++) touch("s", `x${i}`)
  expect(useAgentMruStore.getState().keys).toHaveLength(AGENT_MRU_LIMIT)
})

it("separates the same Agent id across Sessions", () => {
  expect(agentMruKey("host-a", "agent")).not.toBe(agentMruKey("host-b", "agent"))
})

it("orders recent Agents first, skips vanished ones and appends the rest in list order", () => {
  const agents = ["a", "b", "c", "d"]
  expect(orderAgentsByRecency(agents, (x) => x, ["c", "gone", "a"])).toEqual(["c", "a", "b", "d"])
  expect(orderAgentsByRecency(agents, (x) => x, [])).toEqual(agents)
})
