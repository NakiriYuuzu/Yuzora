// A unified diff (-U3) as `git.diff` returns it, parsed for display only.

export type DiffLine =
  | { kind: "context" | "add" | "del"; oldNo: number | null; newNo: number | null; text: string }
  /** `\ No newline at end of file`, which belongs to the line above it. */
  | { kind: "noNewline"; text: string }
export type DiffHunk = { header: string; lines: DiffLine[] }
export type DiffFile = {
  oldPath: string | null
  newPath: string | null
  change: "modified" | "added" | "deleted" | "renamed" | "copied"
  /** `old mode` and `new mode`, when only the permissions changed or moved with the content. */
  mode: [string, string] | null
  binary: boolean
  /** git printed only `* Unmerged path <p>`: a delete/add conflict has no diff to show. */
  unmerged: boolean
  /** A `diff --cc`/`--combined` header, which `git diff -- <path>` prints only for a path that is still unmerged. */
  combined: boolean
  hunks: DiffHunk[]
}

const FILE_START = /^diff --(?:git|cc|combined) /
const COMBINED_START = /^diff --(?:cc|combined) /
const UNMERGED = "* Unmerged path "
const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/
// A merge's combined diff has `@@@ -a,b -c,d +e,f @@@`: one marker column per parent.
const COMBINED_HUNK = /^(@@@+) /

/** The path of a `---`/`+++` line; paths with spaces end in a tab, and /dev/null marks the missing side. */
function headerPath(raw: string): string | null {
  const path = raw.replace(/\t.*$/, "")
  return path === "/dev/null" ? null : path.replace(/^[ab]\//, "")
}

function startFile(line: string): DiffFile {
  // Only a fallback for files without `---`/`+++` lines (empty, renamed, binary): the names are ambiguous when they contain " b/".
  const names = line.replace(/^diff --\S+ /, "")
  const split = names.lastIndexOf(" b/")
  return {
    oldPath: split > 0 ? names.slice(0, split).replace(/^a\//, "") : null,
    newPath: split > 0 ? names.slice(split + 1).replace(/^b\//, "") : null,
    change: "modified",
    mode: null,
    binary: false,
    unmerged: false,
    combined: COMBINED_START.test(line),
    hunks: [],
  }
}

/** Applies a header line (before the first hunk) to its file. */
function readHeader(file: DiffFile, line: string): void {
  if (line.startsWith("--- ")) {
    file.oldPath = headerPath(line.slice(4))
  } else if (line.startsWith("+++ ")) {
    file.newPath = headerPath(line.slice(4))
  } else if (line.startsWith("new file mode ")) {
    file.change = "added"
  } else if (line.startsWith("deleted file mode ")) {
    file.change = "deleted"
  } else if (line.startsWith("rename from ") || line.startsWith("copy from ")) {
    file.change = line.startsWith("rename") ? "renamed" : "copied"
    file.oldPath = line.slice(line.indexOf(" from ") + 6)
  } else if (line.startsWith("rename to ") || line.startsWith("copy to ")) {
    file.newPath = line.slice(line.indexOf(" to ") + 4)
  } else if (line.startsWith("old mode ")) {
    file.mode = [line.slice(9), file.mode?.[1] ?? ""]
  } else if (line.startsWith("new mode ")) {
    file.mode = [file.mode?.[0] ?? "", line.slice(9)]
  } else if (line.startsWith("Binary files ") || line === "GIT binary patch") {
    file.binary = true
  }
}

/**
 * Never throws: a diff the gateway cut short ends mid-hunk, and anything that
 * does not look like a diff simply yields no hunks.
 */
export function parseUnifiedDiff(text: string): DiffFile[] {
  if (text === "") return []
  const lines = text.split("\n")
  // The text ends with a newline, so the last element is not a line.
  if (lines[lines.length - 1] === "") lines.pop()
  const files: DiffFile[] = []
  let file: DiffFile | null = null
  let hunk: DiffHunk | null = null
  let columns = 1
  let oldNo = 0
  let newNo = 0

  for (const line of lines) {
    if (FILE_START.test(line)) {
      file = startFile(line)
      files.push(file)
      hunk = null
      continue
    }
    // Printed for a path with no merged content (UD, DU, AU, UA, DD), possibly ahead of a diff of the file as it is on disk.
    if (line.startsWith(UNMERGED)) {
      const path = line.slice(UNMERGED.length)
      file = { oldPath: path, newPath: path, change: "modified", mode: null, binary: false, unmerged: true, combined: false, hunks: [] }
      files.push(file)
      hunk = null
      continue
    }
    if (hunk && !line.startsWith("@@")) {
      if (line.startsWith("\\")) hunk.lines.push({ kind: "noNewline", text: line })
      else if (columns > 1) hunk.lines.push(combinedLine(line, columns))
      else if (line.startsWith("-")) hunk.lines.push({ kind: "del", oldNo: oldNo++, newNo: null, text: line.slice(1) })
      else if (line.startsWith("+")) hunk.lines.push({ kind: "add", oldNo: null, newNo: newNo++, text: line.slice(1) })
      // A blank context line may have lost its leading space on the way.
      else hunk.lines.push({ kind: "context", oldNo: oldNo++, newNo: newNo++, text: line.startsWith(" ") ? line.slice(1) : line })
      continue
    }
    if (!file) {
      file = { oldPath: null, newPath: null, change: "modified", mode: null, binary: false, unmerged: false, combined: false, hunks: [] }
      files.push(file)
    }
    const start = HUNK.exec(line)
    const combined = start ? null : COMBINED_HUNK.exec(line)
    if (start || combined) {
      columns = combined ? combined[1].length - 1 : 1
      oldNo = Number(start?.[1] ?? 0)
      newNo = Number(start?.[2] ?? 0)
      hunk = { header: line, lines: [] }
      file.hunks.push(hunk)
    } else {
      hunk = null
      readHeader(file, line)
    }
  }
  return files
}

/** One marker column per parent; without a single old and new side there are no line numbers. */
function combinedLine(line: string, columns: number): DiffLine {
  const markers = line.slice(0, columns)
  const text = line.slice(columns)
  if (markers.includes("-")) return { kind: "del", oldNo: null, newNo: null, text }
  if (markers.includes("+")) return { kind: "add", oldNo: null, newNo: null, text }
  return { kind: "context", oldNo: null, newNo: null, text }
}
