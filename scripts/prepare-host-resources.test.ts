import { expect, it } from "vitest"
import { HOST_TARGETS, validateHostArtifact, type HostArtifact } from "./prepare-host-resources"
import { HERDR_RESOURCE_TARGETS } from "./prepare-herdr-resources"

it("pins four Unix targets, helper paths, and the official HERDR hashes", () => {
  expect(Object.keys(HOST_TARGETS)).toHaveLength(4)
  for (const target of Object.keys(HOST_TARGETS) as Array<keyof typeof HOST_TARGETS>) {
    const artifact: HostArtifact = { protocol:1, version:"0.0.9-beta.3", target, helper:{path:`${target}/yuzora-host`,sha256:"a".repeat(64)}, herdr:{path:`${target}/herdr`,sha256:HERDR_RESOURCE_TARGETS[target].files[0].sha256,version:"0.9.0",protocol: 22} }
    expect(() => validateHostArtifact(artifact)).not.toThrow()
    expect(() => validateHostArtifact({...artifact, helper:{...artifact.helper,path:"../../bin/herdr"}})).toThrow("Invalid artifact path")
    expect(() => validateHostArtifact({...artifact, herdr:{...artifact.herdr,sha256:"b".repeat(64)}})).toThrow("Unexpected official HERDR")
  }
})
