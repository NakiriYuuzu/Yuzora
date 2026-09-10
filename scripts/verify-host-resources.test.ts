import { createHash } from "node:crypto"
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { expect, it } from "vitest"
import { HOST_TARGETS, type HostArtifact } from "./prepare-host-resources"
import { HERDR_RESOURCE_TARGETS } from "./prepare-herdr-resources"
import { verifyHostResources } from "./verify-host-resources"

it("blocks missing targets, version drift, changed helper bytes, and Windows executables", async () => {
  const root = await mkdtemp(join(tmpdir(), "yuzora-runtime-payload-"))
  try {
    await expect(verifyHostResources(root, "0.0.9-beta.3")).rejects.toThrow("exactly four")
    const helper = Buffer.from("MZ-not-a-Unix-helper")
    await writeFile(join(root, "LICENSE-HERDR.txt"), await readFile("src-tauri/resources/herdr/LICENSE-HERDR.txt"))
    const artifacts: HostArtifact[] = []
    for (const target of Object.keys(HOST_TARGETS) as Array<keyof typeof HOST_TARGETS>) {
      await mkdir(join(root, target))
      await writeFile(join(root, target, "yuzora-host"), helper)
      await writeFile(join(root, target, "herdr"), "fixture")
      const artifact: HostArtifact = { protocol: 1, version: "0.0.9-beta.3", target, helper: { path: `${target}/yuzora-host`, sha256: createHash("sha256").update(helper).digest("hex") }, herdr: { path: `${target}/herdr`, sha256: HERDR_RESOURCE_TARGETS[target].files[0].sha256, version: "0.9.0", protocol: 22 } }
      artifacts.push(artifact)
      await writeFile(join(root, `${target}.json`), JSON.stringify(artifact))
    }
    await expect(verifyHostResources(root, "different")).rejects.toThrow("version/target mismatch")
    await expect(verifyHostResources(root, "0.0.9-beta.3")).rejects.toThrow("Not a Unix executable")
    const first = artifacts[0]
    await writeFile(join(root, first.helper.path), "changed")
    await expect(verifyHostResources(root, "0.0.9-beta.3")).rejects.toThrow("Runtime hash mismatch")
    await writeFile(join(root, "herdr.exe"), "legacy")
    await expect(verifyHostResources(root, "0.0.9-beta.3")).rejects.toThrow("exactly four")
  } finally { await rm(root, { recursive: true, force: true }) }
})
