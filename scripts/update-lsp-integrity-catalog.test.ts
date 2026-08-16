import { describe, expect, it } from "vitest"
import {
  catalogWritePlan,
  contentTreeManifest,
  githubDigestToSha256,
  proposeBinaryPlatforms,
  proposeNpmPin,
  pypiWheelHashes,
  refuseLocalByteDigest,
  shouldWriteCatalog,
} from "./update-lsp-integrity-catalog"

describe("update-lsp-integrity-catalog", () => {
  it("uses GitHub digest and never a locally computed hash", () => {
    const local = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
    const github = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    expect(refuseLocalByteDigest({ localSha256: local, githubDigest: github })).toBe(
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    )
    expect(refuseLocalByteDigest({ localSha256: local, githubDigest: null })).toBeNull()
    expect(githubDigestToSha256("md5:abc")).toBeNull()
  })

  it("marks a platform unavailable when GitHub digest is missing", () => {
    const proposal = proposeBinaryPlatforms(
      {
        tag_name: "2026-06-29",
        assets: [
          {
            name: "rust-analyzer-aarch64-apple-darwin.gz",
            browser_download_url: "https://example.com/ra.gz",
          },
        ],
      },
      {
        "macos/aarch64": {
          name: "rust-analyzer-aarch64-apple-darwin.gz",
          unpack: "gz",
          executable: "rust-analyzer",
        },
      }
    )
    expect(proposal["macos/aarch64"]).toEqual({
      status: "unavailable",
      reason: "missing reviewed GitHub digest for rust-analyzer-aarch64-apple-darwin.gz",
    })
  })

  it("emits a supported platform only from the release digest field", () => {
    const proposal = proposeBinaryPlatforms(
      {
        assets: [
          {
            name: "marksman-macos",
            browser_download_url:
              "https://github.com/artempyanykh/marksman/releases/download/2026-02-08/marksman-macos",
            digest: "sha256:6a801c17b5ac0dba69787c5282b3b3bd416e66c96253fae098d311c6bbd1833b",
          },
        ],
      },
      {
        "macos/aarch64": { name: "marksman-macos", unpack: "bare", executable: "marksman" },
      }
    )
    expect(proposal["macos/aarch64"]?.status).toBe("supported")
    expect(proposal["macos/aarch64"]?.sha256).toBe(
      "6a801c17b5ac0dba69787c5282b3b3bd416e66c96253fae098d311c6bbd1833b"
    )
  })

  it("requires npm integrity and does not default to latest without metadata", () => {
    expect(proposeNpmPin({ versions: {} })).toMatchObject({ status: "unavailable" })
    expect(
      proposeNpmPin({
        "dist-tags": { latest: "1.2.3" },
        versions: { "1.2.3": { dist: { integrity: "sha512-abc" } } },
      })
    ).toMatchObject({ status: "supported", version: "1.2.3", integrity: "sha512-abc" })
  })

  it("collects only unyanked wheel hashes for pip", () => {
    expect(
      pypiWheelHashes([
        { packagetype: "sdist", digests: { sha256: "sdist" } },
        { packagetype: "bdist_wheel", yanked: true, digests: { sha256: "yanked" } },
        { packagetype: "bdist_wheel", digests: { sha256: "bbbb" } },
        { packagetype: "bdist_wheel", digests: { sha256: "aaaa" } },
      ])
    ).toEqual(["aaaa", "bbbb"])
  })

  it("builds deterministic reviewed content-tree manifests and detects byte changes", () => {
    const encoder = new TextEncoder()
    const files = [
      { path: "pkg/b.js", bytes: encoder.encode("b") },
      { path: "pkg/a.js", bytes: encoder.encode("a") },
    ]
    const first = contentTreeManifest(["pkg"], files, "a".repeat(64))
    const reordered = contentTreeManifest(["pkg"], [...files].reverse(), "a".repeat(64))
    const tampered = contentTreeManifest(
      ["pkg"],
      [{ path: "pkg/a.js", bytes: encoder.encode("evil") }, files[0]],
      "a".repeat(64)
    )
    expect(first).toEqual(reordered)
    expect(first).toMatchObject({ version: 1, roots: ["pkg"], fileCount: 2 })
    expect(first.treeSha256).not.toBe(tampered.treeSha256)
  })

  it("rejects duplicate or unsafe content-tree paths", () => {
    const bytes = new TextEncoder().encode("x")
    expect(() =>
      contentTreeManifest(["pkg"], [
        { path: "pkg/a.js", bytes },
        { path: "pkg/a.js", bytes },
      ])
    ).toThrow("duplicate")
    expect(() => contentTreeManifest(["pkg"], [{ path: "../a.js", bytes }])).toThrow("unsafe")
  })

  it("does not write the catalog unless --write is explicit", () => {
    expect(shouldWriteCatalog([])).toBe(false)
    expect(shouldWriteCatalog(["--dry-run"])).toBe(false)
    expect(shouldWriteCatalog(["--write"])).toBe(true)
    expect(catalogWritePlan([], "src-tauri/lsp-catalog/manifest.json")).toEqual({
      write: false,
      dest: "src-tauri/lsp-catalog/manifest.json",
    })
  })
})
