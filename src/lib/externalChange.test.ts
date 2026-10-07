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

test("coalesced directory reaches clean/dirty descendants, not a shared name prefix", () => {
    const result = handleExternalChange(
        ["/w/target", "/w/target/a.txt"],
        [tab("/w/target/a.txt", false), tab("/w/target/b.txt", true), tab("/w/targetX/a.txt", true)],
        new Set()
    )
    expect(result.reload).toEqual(["/w/target/a.txt"])
    expect(result.markModified).toEqual(["/w/target/b.txt"])
})

test("coalesced directory preserves recently saved suppression and deduplicates split tabs", () => {
    const result = handleExternalChange(
        ["/w/target"],
        [tab("/w/target/a.txt", false), tab("/w/target/a.txt", false), tab("/w/target/saved.txt", true)],
        new Set(["/w/target/saved.txt"])
    )
    expect(result.reload).toEqual(["/w/target/a.txt"])
    expect(result.markModified).toEqual([])
})

test("coalesced Windows directories use canonical case and separator matching", () => {
    const path = String.raw`C:\Work\Target\a.txt`
    const result = handleExternalChange(
        ["c:/work/target/"],
        [tab(path, true), tab(String.raw`C:\Work\TargetX\a.txt`, true)],
        new Set()
    )
    expect(result.markModified).toEqual([path])
    expect(result.reload).toEqual([])
})
