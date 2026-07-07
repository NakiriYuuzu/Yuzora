import { beforeEach, describe, expect, it } from "vitest"

import { usePerfStore } from "@/state/perfStore"

describe("perfStore", () => {
    beforeEach(() => {
        usePerfStore.getState().reset()
    })

    it("starts with a null snapshot", () => {
        expect(usePerfStore.getState().snapshot).toBeNull()
    })

    it("setSnapshot stores the latest sample", () => {
        usePerfStore.getState().setSnapshot({ cpuPercent: 12.5, memoryBytes: 184_000_000 })
        expect(usePerfStore.getState().snapshot).toEqual({
            cpuPercent: 12.5,
            memoryBytes: 184_000_000
        })
    })

    it("reset clears the snapshot back to null", () => {
        usePerfStore.getState().setSnapshot({ cpuPercent: 3, memoryBytes: 1 })
        usePerfStore.getState().reset()
        expect(usePerfStore.getState().snapshot).toBeNull()
    })
})
