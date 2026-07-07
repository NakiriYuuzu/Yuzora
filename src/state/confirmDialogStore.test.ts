import { afterEach, expect, test } from "vitest"

import { useConfirmDialogStore } from "./confirmDialogStore"

afterEach(() => useConfirmDialogStore.setState({ pending: null }))

test("requestUnsavedDecision 設定 pending，respond 後 resolve 並清空", async () => {
    const p = useConfirmDialogStore.getState().requestUnsavedDecision({
        title: "t",
        description: "d",
        saveLabel: "Save"
    })
    expect(useConfirmDialogStore.getState().pending).toMatchObject({
        title: "t",
        description: "d",
        saveLabel: "Save"
    })
    useConfirmDialogStore.getState().respond("discard")
    expect(await p).toBe("discard")
    expect(useConfirmDialogStore.getState().pending).toBeNull()
})

test("respond 無 pending 時為 no-op", () => {
    useConfirmDialogStore.setState({ pending: null })
    expect(() => useConfirmDialogStore.getState().respond("save")).not.toThrow()
})

test("新請求覆蓋前一個未決請求時，前者以 cancel resolve", async () => {
    const store = useConfirmDialogStore.getState()
    const p1 = store.requestUnsavedDecision({ title: "1", description: "d", saveLabel: "Save" })
    const p2 = store.requestUnsavedDecision({ title: "2", description: "d", saveLabel: "Save" })
    expect(await p1).toBe("cancel")
    useConfirmDialogStore.getState().respond("save")
    expect(await p2).toBe("save")
})
