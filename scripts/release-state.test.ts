import { describe, expect, it } from "vitest"

import { resolveReleaseState } from "./release-state"

const sourceSha = "source-sha"
const historicalSha = "published-tag-sha"

function state(overrides: Partial<Parameters<typeof resolveReleaseState>[0]> = {}) {
  return {
    channel: "beta" as const,
    sourceSha,
    tagSha: null,
    release: null,
    ...overrides,
  }
}

describe("release state resolution", () => {
  it("skips a matching-channel published release on a later main commit", () => {
    expect(
      resolveReleaseState(
        state({
          tagSha: historicalSha,
          release: { isDraft: false, isPrerelease: true },
        })
      )
    ).toEqual({ shouldBuild: false, shouldPublishExisting: false })
  })

  it("resumes only a matching draft on the successful CI SHA", () => {
    expect(
      resolveReleaseState(
        state({
          tagSha: sourceSha,
          release: { isDraft: true, isPrerelease: true },
        })
      )
    ).toEqual({ shouldBuild: false, shouldPublishExisting: true })
    expect(() =>
      resolveReleaseState(
        state({
          tagSha: historicalSha,
          release: { isDraft: true, isPrerelease: true },
        })
      )
    ).toThrow("existing draft tag")
  })

  it("resumes a tag without a Release only on the successful CI SHA", () => {
    expect(resolveReleaseState(state({ tagSha: sourceSha }))).toEqual({
      shouldBuild: true,
      shouldPublishExisting: false,
    })
    expect(() => resolveReleaseState(state({ tagSha: historicalSha }))).toThrow(
      "existing tag without a Release"
    )
  })

  it("rejects wrong-channel releases and handles an absent state", () => {
    expect(() =>
      resolveReleaseState(
        state({
          tagSha: sourceSha,
          release: { isDraft: false, isPrerelease: false },
        })
      )
    ).toThrow("does not match beta channel")
    expect(resolveReleaseState(state())).toEqual({
      shouldBuild: true,
      shouldPublishExisting: false,
    })
  })
})
