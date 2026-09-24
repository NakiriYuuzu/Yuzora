import { expect, it } from "vitest"
import { HOST_TARGETS, validateHostArtifact, type HostArtifact } from "./prepare-host-resources"
import { HERDR_RESOURCE_TARGETS } from "./prepare-herdr-resources"

it("pins five targets, helper paths, and complete official HERDR packages", () => {
  expect(Object.keys(HOST_TARGETS)).toHaveLength(5)
  for (const target of Object.keys(HOST_TARGETS) as Array<keyof typeof HOST_TARGETS>) {
    const suffix = target.startsWith("windows-") ? ".exe" : ""
    const files = HERDR_RESOURCE_TARGETS[target].files
    const artifact: HostArtifact = { protocol:1, version:"0.0.9-beta.3", target, helper:{path:`${target}/yuzora-host${suffix}`,sha256:"a".repeat(64)}, herdr:{path:`${target}/herdr${suffix}`,sha256:files.find(file => file.path === `herdr${suffix}`)!.sha256,version:"0.9.1",protocol: 22}, files:files.filter(file => file.path !== `herdr${suffix}`).map(file => ({path:`${target}/${file.path}`,sha256:file.sha256})) }
    expect(() => validateHostArtifact(artifact)).not.toThrow()
    expect(() => validateHostArtifact({...artifact, helper:{...artifact.helper,path:"../../bin/herdr"}})).toThrow("Invalid artifact path")
    expect(() => validateHostArtifact({...artifact, herdr:{...artifact.herdr,sha256:"b".repeat(64)}})).toThrow("Unexpected official HERDR")
    if (target === "windows-x86_64") {
      expect(() => validateHostArtifact({...artifact,files:[]})).toThrow("auxiliary")
      expect(() => validateHostArtifact({...artifact,files:[...artifact.files!,artifact.files![0]]})).toThrow("auxiliary")
    }
  }
})
