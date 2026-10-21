import { describe, expect, it } from "vitest"
import { parseUnifiedDiff } from "./diff"

const lines = (...rows: string[]) => `${rows.join("\n")}\n`

describe("parseUnifiedDiff", () => {
  it("numbers the old and new side across several hunks", () => {
    const [file] = parseUnifiedDiff(lines(
      "diff --git a/src/a.ts b/src/a.ts",
      "index 1111111..2222222 100644",
      "--- a/src/a.ts",
      "+++ b/src/a.ts",
      "@@ -1,4 +1,5 @@",
      " line1",
      "-line2",
      "+line2 changed",
      "+line2b",
      " line3",
      " line4",
      "@@ -20,3 +21,3 @@ function foo() {",
      " ctx",
      "-old",
      "+new",
      " end",
    ))
    expect(file).toMatchObject({ oldPath: "src/a.ts", newPath: "src/a.ts", change: "modified", binary: false })
    expect(file.hunks.map((hunk) => hunk.header)).toEqual(["@@ -1,4 +1,5 @@", "@@ -20,3 +21,3 @@ function foo() {"])
    const numbers = (index: number) => file.hunks[index].lines.map((line) => (line.kind === "noNewline" ? line.kind : [line.kind, line.oldNo, line.newNo, line.text]))
    expect(numbers(0)).toEqual([
      ["context", 1, 1, "line1"],
      ["del", 2, null, "line2"],
      ["add", null, 2, "line2 changed"],
      ["add", null, 3, "line2b"],
      ["context", 3, 4, "line3"],
      ["context", 4, 5, "line4"],
    ])
    expect(numbers(1)).toEqual([
      ["context", 20, 21, "ctx"],
      ["del", 21, null, "old"],
      ["add", null, 22, "new"],
      ["context", 22, 23, "end"],
    ])
  })

  it("keeps the no-newline marker with the line it follows", () => {
    const [file] = parseUnifiedDiff(lines(
      "diff --git a/n.txt b/n.txt",
      "--- a/n.txt",
      "+++ b/n.txt",
      "@@ -1,2 +1,2 @@",
      " a",
      "-b",
      "\\ No newline at end of file",
      "+b",
      "\\ No newline at end of file",
    ))
    expect(file.hunks[0].lines.map((line) => line.kind)).toEqual(["context", "del", "noNewline", "add", "noNewline"])
    expect(file.hunks[0].lines[2]).toEqual({ kind: "noNewline", text: "\\ No newline at end of file" })
    // The marker is not a line: the next number is unaffected.
    expect(file.hunks[0].lines[3]).toMatchObject({ kind: "add", newNo: 2 })
  })

  it("reads a rename without content changes from its header", () => {
    const files = parseUnifiedDiff(lines(
      "diff --git a/old name.txt b/new name.txt",
      "similarity index 100%",
      "rename from old name.txt",
      "rename to new name.txt",
    ))
    expect(files).toEqual([{ oldPath: "old name.txt", newPath: "new name.txt", change: "renamed", mode: null, binary: false, unmerged: false, combined: false, hunks: [] }])
  })

  it("reads a rename that also edits the file and changes its mode", () => {
    const [file] = parseUnifiedDiff(lines(
      "diff --git a/a.sh b/b.sh",
      "old mode 100644",
      "new mode 100755",
      "similarity index 90%",
      "rename from a.sh",
      "rename to b.sh",
      "index 1111111..2222222",
      "--- a/a.sh",
      "+++ b/b.sh",
      "@@ -1 +1 @@",
      "-x",
      "+y",
    ))
    expect(file).toMatchObject({ oldPath: "a.sh", newPath: "b.sh", change: "renamed", mode: ["100644", "100755"] })
    expect(file.hunks[0].lines).toEqual([
      { kind: "del", oldNo: 1, newNo: null, text: "x" },
      { kind: "add", oldNo: null, newNo: 1, text: "y" },
    ])
  })

  it("tells a new file and a deleted file by their /dev/null side", () => {
    const [added, deleted] = parseUnifiedDiff(lines(
      "diff --git a/new.txt b/new.txt",
      "new file mode 100644",
      "index 0000000..1111111",
      "--- /dev/null",
      "+++ b/new.txt",
      "@@ -0,0 +1,2 @@",
      "+one",
      "+two",
      "diff --git a/gone.txt b/gone.txt",
      "deleted file mode 100644",
      "index 1111111..0000000",
      "--- a/gone.txt",
      "+++ /dev/null",
      "@@ -1 +0,0 @@",
      "-bye",
    ))
    expect(added).toMatchObject({ oldPath: null, newPath: "new.txt", change: "added" })
    expect(added.hunks[0].lines).toEqual([
      { kind: "add", oldNo: null, newNo: 1, text: "one" },
      { kind: "add", oldNo: null, newNo: 2, text: "two" },
    ])
    expect(deleted).toMatchObject({ oldPath: "gone.txt", newPath: null, change: "deleted" })
    expect(deleted.hunks[0].lines).toEqual([{ kind: "del", oldNo: 1, newNo: null, text: "bye" }])
  })

  it("marks binary files and has no hunks for them", () => {
    expect(parseUnifiedDiff(lines("diff --git a/x.png b/x.png", "index 1111111..2222222 100644", "Binary files a/x.png and b/x.png differ"))).toMatchObject([{ binary: true, hunks: [] }])
    expect(parseUnifiedDiff(lines("diff --git a/x.bin b/x.bin", "index 1..2", "GIT binary patch", "literal 3", "Kc$@"))[0].binary).toBe(true)
  })

  it("returns nothing for an empty diff", () => {
    expect(parseUnifiedDiff("")).toEqual([])
  })

  it("keeps lines that only look like file headers inside a hunk", () => {
    const [file] = parseUnifiedDiff(lines(
      "diff --git a/q.sql b/q.sql",
      "--- a/q.sql",
      "+++ b/q.sql",
      "@@ -1,2 +1,2 @@",
      "--- a comment",
      "+++ another",
    ))
    expect(file.oldPath).toBe("q.sql")
    expect(file.hunks[0].lines).toEqual([
      { kind: "del", oldNo: 1, newNo: null, text: "-- a comment" },
      { kind: "add", oldNo: null, newNo: 1, text: "++ another" },
    ])
  })

  it("strips the tab git adds after paths with spaces", () => {
    const [file] = parseUnifiedDiff(lines("diff --git a/my file.txt b/my file.txt", "--- a/my file.txt\t", "+++ b/my file.txt\t", "@@ -1 +1 @@", "-a", "+b"))
    expect(file).toMatchObject({ oldPath: "my file.txt", newPath: "my file.txt" })
  })

  it("survives a diff cut off in the middle of a hunk", () => {
    const [file] = parseUnifiedDiff(lines("diff --git a/big.txt b/big.txt", "--- a/big.txt", "+++ b/big.txt", "@@ -1,50 +1,50 @@", " one", "-two", "+TWO"))
    expect(file.hunks).toHaveLength(1)
    expect(file.hunks[0].lines).toHaveLength(3)
  })

  it("shows a merge's combined diff line by line without numbers", () => {
    const [file] = parseUnifiedDiff(lines(
      "diff --cc f.txt",
      "index 1111111,2222222..3333333",
      "--- a/f.txt",
      "+++ b/f.txt",
      "@@@ -1,3 -1,3 +1,4 @@@",
      "  base",
      "- ours",
      " -theirs",
      "++merged",
    ))
    expect(file.hunks[0].lines).toEqual([
      { kind: "context", oldNo: null, newNo: null, text: "base" },
      { kind: "del", oldNo: null, newNo: null, text: "ours" },
      { kind: "del", oldNo: null, newNo: null, text: "theirs" },
      { kind: "add", oldNo: null, newNo: null, text: "merged" },
    ])
  })

  it("recognises a delete/modify conflict, which git prints as a single line", () => {
    expect(parseUnifiedDiff(lines("* Unmerged path g.txt"))).toEqual([{ oldPath: "g.txt", newPath: "g.txt", change: "modified", mode: null, binary: false, unmerged: true, combined: false, hunks: [] }])
    expect(parseUnifiedDiff(lines("* Unmerged path my dir/my file.txt"))).toMatchObject([{ newPath: "my dir/my file.txt", unmerged: true }])
  })

  it("keeps the diff that follows an unmerged path, as when the file was edited after the conflict", () => {
    const files = parseUnifiedDiff(lines(
      "* Unmerged path g.txt",
      "diff --git a/g.txt b/g.txt",
      "--- a/g.txt",
      "+++ b/g.txt",
      "@@ -1 +1 @@",
      "-a",
      "+b",
    ))
    expect(files.map((file) => file.unmerged)).toEqual([true, false])
    expect(files[1].hunks[0].lines).toEqual([
      { kind: "del", oldNo: 1, newNo: null, text: "a" },
      { kind: "add", oldNo: null, newNo: 1, text: "b" },
    ])
  })

  it("reads the bare header git prints for a conflict that matches one side as a file without hunks", () => {
    // Why the diff screen needs the Conflicted group's word for it: the text itself does not say "unmerged".
    expect(parseUnifiedDiff(lines("diff --cc f.txt", "index daf31e1,594dc4f..0000000", "--- a/f.txt", "+++ b/f.txt"))).toEqual([
      { oldPath: "f.txt", newPath: "f.txt", change: "modified", mode: null, binary: false, unmerged: false, combined: true, hunks: [] },
    ])
    expect(parseUnifiedDiff(lines("diff --cc f.txt", "index daf31e1,594dc4f..0000000", "deleted file mode 100644,100644", "--- a/f.txt", "+++ /dev/null"))).toEqual([
      { oldPath: "f.txt", newPath: null, change: "deleted", mode: null, binary: false, unmerged: false, combined: true, hunks: [] },
    ])
  })

  it("does not take a combined diff of a content conflict for an unmerged path", () => {
    const [file] = parseUnifiedDiff(lines("diff --cc h.txt", "--- a/h.txt", "+++ b/h.txt", "@@@ -1,2 -1,2 +1,6 @@@", "  base"))
    expect(file.unmerged).toBe(false)
  })

  it("tells a combined header (`--cc`, `--combined`) from a plain one, which says nothing about a conflict", () => {
    const combined = (text: string) => parseUnifiedDiff(lines(...text.split("\n"))).map((file) => file.combined)
    expect(combined("diff --cc f.txt\nindex daf31e1,594dc4f..0000000")).toEqual([true])
    expect(combined("diff --combined f.txt\nindex daf31e1,594dc4f..0000000")).toEqual([true])
    expect(combined("diff --git a/f.txt b/f.txt\nold mode 100644\nnew mode 100755")).toEqual([false])
    expect(combined("diff --git a/f.txt b/f.txt\ndiff --cc g.txt\n* Unmerged path h.txt")).toEqual([false, true, false])
  })
})
