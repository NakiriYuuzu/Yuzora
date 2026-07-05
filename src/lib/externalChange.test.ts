import { expect, test } from "vitest"
import { handleExternalChange } from "./externalChange"

const tab = (path: string, dirty: boolean) => ({
    path,
    name: path.split("/").pop() ?? "",
    dirty,
    externallyModified: false
})

test("clean tab 進 reload、dirty tab 進 markModified、未開啟路徑忽略", () => {
    const result = handleExternalChange(
        ["/w/a.ts", "/w/b.ts", "/w/other.ts"],
        [tab("/w/a.ts", false), tab("/w/b.ts", true)],
        new Set()
    )
    expect(result.reload).toEqual(["/w/a.ts"])
    expect(result.markModified).toEqual(["/w/b.ts"])
})

test("剛存檔的路徑被抑制", () => {
    const result = handleExternalChange(
        ["/w/a.ts"],
        [tab("/w/a.ts", false)],
        new Set(["/w/a.ts"])
    )
    expect(result.reload).toEqual([])
    expect(result.markModified).toEqual([])
})
