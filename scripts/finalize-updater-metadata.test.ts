import { describe, expect, it } from "vitest"
import { finalizeUpdaterMetadata } from "./finalize-updater-metadata"

const msiUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.2/Yuzora_0.0.2_x64_en-US.msi"
const nsisUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.2/Yuzora_0.0.2_x64-setup.exe"
const macUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.2/Yuzora_universal.app.tar.gz"

function metadata() {
  return {
    version: "0.0.2",
    platforms: {
      "windows-x86_64": { url: msiUrl, signature: "msi-signature" },
      "windows-x86_64-msi": { url: msiUrl, signature: "msi-signature" },
      "windows-x86_64-nsis": { url: nsisUrl, signature: "nsis-signature" },
      "darwin-aarch64": { url: macUrl, signature: "mac-signature" },
    },
  }
}

const assets = [
  "latest.json",
  "Yuzora_0.0.2_x64_en-US.msi",
  "Yuzora_0.0.2_x64_en-US.msi.sig",
  "Yuzora_0.0.2_x64-setup.exe",
  "Yuzora_0.0.2_x64-setup.exe.sig",
  "Yuzora_universal.app.tar.gz",
  "Yuzora_universal.app.tar.gz.sig",
]

describe("finalizeUpdaterMetadata", () => {
  it("removes NSIS platform entries while keeping MSI as the Windows OTA target", () => {
    const finalized = finalizeUpdaterMetadata(metadata(), assets, "0.0.2")

    expect(finalized.platforms["windows-x86_64"]).toEqual({
      url: msiUrl,
      signature: "msi-signature",
    })
    expect(finalized.platforms["windows-x86_64-msi"]).toBeDefined()
    expect(finalized.platforms["windows-x86_64-nsis"]).toBeUndefined()
  })

  it("rejects a generic Windows target that does not use MSI", () => {
    const input = metadata()
    input.platforms["windows-x86_64"] = { url: nsisUrl, signature: "nsis-signature" }

    expect(() => finalizeUpdaterMetadata(input, assets, "0.0.2")).toThrow(
      "windows-x86_64 must point to an MSI asset"
    )
  })

  it("rejects metadata whose artifact or detached signature is absent", () => {
    expect(() =>
      finalizeUpdaterMetadata(
        metadata(),
        assets.filter((name) => name !== "Yuzora_universal.app.tar.gz.sig"),
        "0.0.2"
      )
    ).toThrow("missing signature asset Yuzora_universal.app.tar.gz.sig")
  })
})
