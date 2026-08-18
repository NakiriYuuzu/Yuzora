export type ReleaseChannel = "stable" | "beta"

const number = "(?:0|[1-9]\\d*)"
const stablePattern = new RegExp(`^${number}\\.${number}\\.${number}$`)
const betaPattern = new RegExp(`^${number}\\.${number}\\.${number}-beta\\.[1-9]\\d*$`)

export function classifyReleaseVersion(version: string): ReleaseChannel | null {
  if (stablePattern.test(version)) return "stable"
  if (betaPattern.test(version)) return "beta"
  return null
}

export function assertReleaseVersion(version: string): ReleaseChannel {
  const channel = classifyReleaseVersion(version)
  if (!channel) {
    throw new Error(
      `version ${version} must be stable X.Y.Z or beta X.Y.Z-beta.N (N starts at 1)`
    )
  }
  return channel
}

export function versionFromTag(tag: string): string {
  const version = tag.startsWith("v") ? tag.slice(1) : tag
  assertReleaseVersion(version)
  return version
}
