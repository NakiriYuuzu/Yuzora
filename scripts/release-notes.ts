import { extractReleaseNotes } from "../src/lib/releaseNotes"

export function releaseNotesForTag(changelog: string, tag: string): string {
  const version = tag.replace(/^v/, "")
  const notes = extractReleaseNotes(changelog, version)
  if (!notes) throw new Error(`CHANGELOG.md must include version ${version}`)
  return notes
}

if (import.meta.main) {
  const tag = process.argv[2] ?? process.env.GITHUB_REF_NAME
  if (!tag) throw new Error("usage: bun scripts/release-notes.ts <tag> [output-path]")

  const notes = releaseNotesForTag(await Bun.file("CHANGELOG.md").text(), tag)
  const outputPath = process.argv[3]
  if (outputPath) await Bun.write(outputPath, `${notes}\n`)

  console.log(`User-facing release notes verified for ${tag}`)
}
