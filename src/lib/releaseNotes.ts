export type ReleaseNoteLine = {
  kind: "heading" | "item" | "text"
  text: string
}

export function extractReleaseNotes(markdown: string, rawVersion: string): string | null {
  const version = rawVersion.replace(/^v/, "")
  const lines = markdown.split(/\r?\n/)
  const header = `## [${version}]`
  const start = lines.findIndex((line) => line === header || line.startsWith(`${header} - `))
  if (start === -1) return null

  const nextVersion = lines.findIndex((line, index) => index > start && line.startsWith("## ["))
  const notes = lines.slice(start + 1, nextVersion === -1 ? undefined : nextVersion).join("\n").trim()
  return notes || null
}

export function parseReleaseNoteLines(markdown: string): ReleaseNoteLine[] {
  return markdown.split(/\r?\n/).flatMap<ReleaseNoteLine>((rawLine) => {
    const line = rawLine.trim()
    if (!line) return []
    if (line.startsWith("### ")) return [{ kind: "heading", text: line.slice(4).trim() }]
    if (line.startsWith("- ")) return [{ kind: "item", text: line.slice(2).trim() }]
    return [{ kind: "text", text: line }]
  })
}
