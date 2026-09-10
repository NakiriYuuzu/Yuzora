import { execFileSync, spawnSync } from "node:child_process"
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import { expect, it } from "vitest"
import { prepareUpdaterMetadata } from "./prepare-updater-metadata"

it("collects raw Tauri macOS archives into versioned Apple Silicon metadata inputs", () => {
  const workflow = JSON.parse(execFileSync("bun", ["-e", "console.log(JSON.stringify(Bun.YAML.parse(await Bun.file('.github/workflows/release.yml').text())))"], { encoding: "utf8" })) as {
    jobs: { build: { steps: { name?: string; run?: string }[] } }
  }
  const collect = workflow.jobs.build.steps.find(step => step.name === "Collect verified local installer artifacts")!.run!
  const root = mkdtempSync(join(tmpdir(), "yuzora-macos-assets-"))
  try {
    const bundle = join(root, "src-tauri/target/aarch64-apple-darwin/release/bundle")
    mkdirSync(join(bundle, "macos"), { recursive: true })
    mkdirSync(join(bundle, "dmg"), { recursive: true })
    writeFileSync(join(root, "package.json"), JSON.stringify({ version: "0.0.9" }))
    writeFileSync(join(bundle, "dmg/Yuzora_0.0.9_aarch64.dmg"), "installer fixture")
    writeFileSync(join(bundle, "macos/Yuzora.app.tar.gz"), "archive fixture")
    writeFileSync(join(bundle, "macos/Yuzora.app.tar.gz.sig"), "mac-signature")
    // This test covers collection/metadata, while macOS CI checks the real Mach-O.
    const bin = join(root, "bin")
    mkdirSync(bin)
    writeFileSync(join(bin, "lipo"), '#!/bin/sh\nprintf "arm64\\n"\n')
    chmodSync(join(bin, "lipo"), 0o755)
    const result = spawnSync("bash", ["-c", collect.replaceAll("${{ matrix.artifact_name }}", "macos")], {
      cwd: root,
      env: { ...process.env, IS_BETA: "false", PATH: `${bin}${delimiter}${process.env.PATH}` },
      encoding: "utf8"
    })
    expect(result.status, result.stderr).toBe(0)
    const assets = join(root, "release-assets")
    expect(readdirSync(assets).sort()).toEqual([
      "Yuzora_0.0.9_aarch64.app.tar.gz", "Yuzora_0.0.9_aarch64.app.tar.gz.sig", "Yuzora_0.0.9_aarch64.dmg"
    ])
    writeFileSync(join(assets, "Yuzora_0.0.9_x64_en-US.msi"), "installer fixture")
    writeFileSync(join(assets, "Yuzora_0.0.9_x64_en-US.msi.sig"), "windows-signature")
    const metadata = prepareUpdaterMetadata("v0.0.9", "NakiriYuuzu/Yuzora", "Release notes", readdirSync(assets), assets)
    expect(Object.keys(metadata.platforms).sort()).toEqual(["darwin-aarch64", "windows-x86_64"])
    expect(metadata.platforms["darwin-aarch64"].url).toContain("Yuzora_0.0.9_aarch64.app.tar.gz")
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
