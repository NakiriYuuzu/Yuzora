import type { ReleaseChannel } from "./release-version"

export interface ReleaseStateInput {
  channel: ReleaseChannel
  sourceSha: string
  tagSha: string | null
  release: {
    isDraft: boolean
    isPrerelease: boolean
  } | null
}

export interface ReleaseStateDecision {
  shouldBuild: boolean
  shouldPublishExisting: boolean
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

export function resolveReleaseState(input: ReleaseStateInput): ReleaseStateDecision {
  const expectedPrerelease = input.channel === "beta"

  if (input.release) {
    assert(input.tagSha, "a GitHub Release must have a matching tag")
    assert(
      input.release.isPrerelease === expectedPrerelease,
      `existing release prerelease state does not match ${input.channel} channel`
    )

    // A published release is immutable. Later main commits may retain the same
    // product version while carrying unrelated changes, so do not compare its
    // historical tag SHA to the current successful CI SHA.
    if (!input.release.isDraft) {
      return { shouldBuild: false, shouldPublishExisting: false }
    }

    assert(
      input.tagSha === input.sourceSha,
      "existing draft tag must point to the successful main CI SHA"
    )
    return { shouldBuild: false, shouldPublishExisting: true }
  }

  if (input.tagSha) {
    assert(
      input.tagSha === input.sourceSha,
      "existing tag without a Release must point to the successful main CI SHA"
    )
  }

  return { shouldBuild: true, shouldPublishExisting: false }
}

if (import.meta.main) {
  try {
    const raw = process.env.YUZORA_RELEASE_STATE
    if (!raw) throw new Error("YUZORA_RELEASE_STATE is required")
    console.log(JSON.stringify(resolveReleaseState(JSON.parse(raw) as ReleaseStateInput)))
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  }
}
