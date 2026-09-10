import { mkdtempSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { describe, expect, it } from "vitest"

import { prepareUpdaterMetadata } from "./prepare-updater-metadata"

const tag = "v0.0.9"
const version = "0.0.9"
const archive = `Yuzora_${version}_aarch64.app.tar.gz`
const msi = `Yuzora_${version}_x64_en-US.msi`

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "yuzora-updater-metadata-"))
  writeFileSync(join(directory, `${archive}.sig`), "mac-signature\n")
  writeFileSync(join(directory, `${msi}.sig`), "windows-signature\n")
  return directory
}

describe("prepare updater metadata", () => {
  it("builds signed Apple Silicon and MSI metadata without an Intel macOS target", () => {
    const metadata = prepareUpdaterMetadata(
      tag,
      "NakiriYuuzu/Yuzora",
      "Stable notes",
      [archive, `${archive}.sig`, msi, `${msi}.sig`, "Yuzora-windows-x64.msi"],
      fixture(),
      "2026-08-18T00:00:00.000Z"
    )

    expect(metadata).toMatchObject({
      version,
      notes: "Stable notes",
      platforms: {
        "darwin-aarch64": {
          url: expect.stringContaining(`/releases/download/${tag}/${archive}`),
          signature: "mac-signature",
        },
        "windows-x86_64": {
          url: expect.stringContaining(`/releases/download/${tag}/${msi}`),
          signature: "windows-signature",
        },
      },
    })
    expect(Object.keys(metadata.platforms).sort()).toEqual(["darwin-aarch64", "windows-x86_64"])
  })

  it("rejects a universal archive for the Apple Silicon release", () => {
    const universal = `Yuzora_${version}_universal.app.tar.gz`
    expect(() => prepareUpdaterMetadata(tag, "NakiriYuuzu/Yuzora", "Notes", [universal, `${universal}.sig`, msi, `${msi}.sig`], fixture()))
      .toThrow("must target Apple Silicon")
  })

  it("rejects missing updater signatures and ambiguous updater artifacts", () => {
    const signatures = fixture()
    expect(() =>
      prepareUpdaterMetadata(
        tag,
        "NakiriYuuzu/Yuzora",
        "Stable notes",
        [archive, `${archive}.sig`, msi],
        signatures,
        "2026-08-18T00:00:00.000Z"
      )
    ).toThrow("missing updater signature")

    expect(() =>
      prepareUpdaterMetadata(
        tag,
        "NakiriYuuzu/Yuzora",
        "Stable notes",
        [archive, `${archive}.sig`, `${archive}.copy.app.tar.gz`, msi, `${msi}.sig`],
        signatures,
        "2026-08-18T00:00:00.000Z"
      )
    ).toThrow("exactly one macOS updater archive")
  })
})
