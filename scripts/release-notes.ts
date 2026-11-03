import { extractReleaseNotes } from "../src/lib/releaseNotes"
import { versionFromTag } from "./release-version"

// The release body carries both changelogs, English first, each under its own heading.
// The headings are "### " because that is the only level the installed apps' update
// notes render (parseReleaseNoteLines); a "## " line would show up with its hash marks.
export const CHANGELOGS = [
  { path: "CHANGELOG.md", title: "English" },
  { path: "CHANGELOG.zh-TW.md", title: "繁體中文" },
] as const

export type Changelogs = Record<(typeof CHANGELOGS)[number]["path"], string>

export function releaseNotesForTag(changelogs: Changelogs, tag: string): string {
  const version = versionFromTag(tag)
  return CHANGELOGS.map(({ path, title }) => {
    const notes = extractReleaseNotes(changelogs[path], version)
    if (!notes) throw new Error(`${path} must include version ${version}`)
    return `### ${title}\n\n${notes}`
  }).join("\n\n")
}

export async function readChangelogs(): Promise<Changelogs> {
  const entries = await Promise.all(
    CHANGELOGS.map(async ({ path }) => [path, await Bun.file(path).text()] as const)
  )
  return Object.fromEntries(entries) as Changelogs
}

if (import.meta.main) {
  const tag = process.argv[2] ?? process.env.GITHUB_REF_NAME
  if (!tag) throw new Error("usage: bun scripts/release-notes.ts <tag> [output-path]")

  const notes = releaseNotesForTag(await readChangelogs(), tag)
  const outputPath = process.argv[3]
  if (outputPath) await Bun.write(outputPath, `${notes}\n`)

  console.log(`User-facing release notes verified for ${tag}`)
}
