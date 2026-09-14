import { describe, expect, it } from "vitest"
import { finalizeUpdaterMetadata } from "./finalize-updater-metadata"

const msiUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.9/Yuzora_0.0.9_x64_en-US.msi"
const nsisUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.9/Yuzora_0.0.9_x64-setup.exe"
const macUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.9/Yuzora_0.0.9_aarch64.app.tar.gz"
const linuxUrl =
  "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.0.9/Yuzora_0.0.9_amd64.AppImage"

function metadata() {
  return {
    version: "0.0.9",
    notes: "### 改善\n\n- 可直接在設定中檢查更新。",
    platforms: {
      "windows-x86_64": { url: msiUrl, signature: "msi-signature" },
      "windows-x86_64-msi": { url: msiUrl, signature: "msi-signature" },
      "windows-x86_64-nsis": { url: nsisUrl, signature: "nsis-signature" },
      "darwin-aarch64": { url: macUrl, signature: "mac-signature" },
      "darwin-x86_64": { url: macUrl, signature: "mac-signature" },
      "linux-x86_64": { url: linuxUrl, signature: "linux-signature" },
    },
  }
}

const assets = [
  "latest.json",
  "Yuzora_0.0.9_x64_en-US.msi",
  "Yuzora_0.0.9_x64_en-US.msi.sig",
  "Yuzora_0.0.9_x64-setup.exe",
  "Yuzora_0.0.9_x64-setup.exe.sig",
  "Yuzora_0.0.9_aarch64.app.tar.gz",
  "Yuzora_0.0.9_aarch64.app.tar.gz.sig",
]

describe("finalizeUpdaterMetadata", () => {
  it("rejects updater metadata without user-facing release notes", () => {
    expect(() =>
      finalizeUpdaterMetadata({ ...metadata(), notes: undefined }, assets, "0.0.9")
    ).toThrow("updater notes are required")
  })

  it("removes unsupported Intel macOS, Linux and NSIS entries while keeping Windows MSI", () => {
    const finalized = finalizeUpdaterMetadata(metadata(), assets, "0.0.9")

    expect(finalized.platforms["windows-x86_64"]).toEqual({
      url: msiUrl,
      signature: "msi-signature",
    })
    expect(finalized.platforms["windows-x86_64-msi"]).toBeDefined()
    expect(finalized.platforms["windows-x86_64-nsis"]).toBeUndefined()
    expect(finalized.platforms["linux-x86_64"]).toBeUndefined()
    expect(finalized.platforms["darwin-x86_64"]).toBeUndefined()
    expect(finalized.notes).toBe("### 改善\n\n- 可直接在設定中檢查更新。")
  })

  it("rejects a generic Windows target that does not use MSI", () => {
    const input = metadata()
    input.platforms["windows-x86_64"] = { url: nsisUrl, signature: "nsis-signature" }

    expect(() => finalizeUpdaterMetadata(input, assets, "0.0.9")).toThrow(
      "windows-x86_64 must point to an MSI asset"
    )
  })

  it("rejects metadata whose artifact or detached signature is absent", () => {
    expect(() =>
      finalizeUpdaterMetadata(
        metadata(),
        assets.filter((name) => name !== "Yuzora_0.0.9_aarch64.app.tar.gz.sig"),
        "0.0.9"
      )
    ).toThrow("missing signature asset Yuzora_0.0.9_aarch64.app.tar.gz.sig")
  })

  it("binds URLs to the expected repository and release tag when context is supplied", () => {
    expect(() =>
      finalizeUpdaterMetadata(metadata(), assets, "0.0.9", {
        repository: "NakiriYuuzu/Yuzora",
        tag: "v0.0.8",
      })
    ).toThrow("URL must reference NakiriYuuzu/Yuzora@v0.0.8")
  })

  it("rejects metadata whose signature differs from the detached signature", () => {
    expect(() =>
      finalizeUpdaterMetadata(metadata(), assets, "0.0.9", {
        repository: "NakiriYuuzu/Yuzora",
        tag: "v0.0.9",
        signatures: new Map([["Yuzora_0.0.9_x64_en-US.msi", "different-signature"]]),
      })
    ).toThrow("signature does not match Yuzora_0.0.9_x64_en-US.msi.sig")
  })
})
