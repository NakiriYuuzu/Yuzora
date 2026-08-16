// Reviewable catalog-update helper for managed LSP identities.
//
// Default: print a proposal. Never trusts locally downloaded bytes as the
// expected digest. Writes reviewed files only when `--write` is passed.

import { createHash } from "node:crypto"

export type BinaryPlatformProposal = {
  status: "supported" | "unavailable"
  url?: string
  sha256?: string
  unpack?: "gz" | "bare" | "zip"
  executable?: string
  reason?: string
}

export type GitHubAsset = {
  name: string
  browser_download_url?: string
  digest?: string | null
}

export type GitHubRelease = {
  tag_name?: string
  assets?: GitHubAsset[]
}

export type NpmPackument = {
  "dist-tags"?: { latest?: string }
  versions?: Record<
    string,
    {
      dist?: { integrity?: string; tarball?: string }
    }
  >
}

export type PypiFile = {
  packagetype?: string
  yanked?: boolean
  digests?: { sha256?: string }
}

export type ContentTreeFile = {
  path: string
  bytes: Uint8Array
}

export type ContentTreeManifest = {
  version: 1
  roots: string[]
  fileCount: number
  treeSha256: string
  artifactSha256?: string
}

const BINARY_ROUTES = [
  {
    language: "rust",
    serverId: "rust-analyzer",
    repo: "rust-lang/rust-analyzer",
    version: "2026-06-29",
    assets: {
      "macos/aarch64": {
        name: "rust-analyzer-aarch64-apple-darwin.gz",
        unpack: "gz" as const,
        executable: "rust-analyzer",
      },
      "macos/x86_64": {
        name: "rust-analyzer-x86_64-apple-darwin.gz",
        unpack: "gz" as const,
        executable: "rust-analyzer",
      },
      "linux/aarch64": {
        name: "rust-analyzer-aarch64-unknown-linux-gnu.gz",
        unpack: "gz" as const,
        executable: "rust-analyzer",
      },
      "linux/x86_64": {
        name: "rust-analyzer-x86_64-unknown-linux-gnu.gz",
        unpack: "gz" as const,
        executable: "rust-analyzer",
      },
      "windows/aarch64": {
        name: "rust-analyzer-aarch64-pc-windows-msvc.zip",
        unpack: "zip" as const,
        executable: "rust-analyzer.exe",
      },
      "windows/x86_64": {
        name: "rust-analyzer-x86_64-pc-windows-msvc.zip",
        unpack: "zip" as const,
        executable: "rust-analyzer.exe",
      },
    },
  },
]

export function githubDigestToSha256(digest: string | null | undefined): string | null {
  if (!digest) return null
  const match = /^sha256:([a-fA-F0-9]{64})$/.exec(digest.trim())
  return match ? match[1].toLowerCase() : null
}

export function proposeBinaryPlatforms(
  release: GitHubRelease,
  assets: Record<string, { name: string; unpack: "gz" | "bare" | "zip"; executable: string }>
): Record<string, BinaryPlatformProposal> {
  const out: Record<string, BinaryPlatformProposal> = {}
  for (const [platform, spec] of Object.entries(assets)) {
    const asset = (release.assets ?? []).find((item) => item.name === spec.name)
    const sha256 = githubDigestToSha256(asset?.digest)
    const url = asset?.browser_download_url
    if (!asset || !url || !sha256) {
      out[platform] = {
        status: "unavailable",
        reason: `missing reviewed GitHub digest for ${spec.name}`,
      }
      continue
    }
    out[platform] = {
      status: "supported",
      url,
      sha256,
      unpack: spec.unpack,
      executable: spec.executable,
    }
  }
  return out
}

export function refuseLocalByteDigest(options: {
  localSha256?: string
  githubDigest?: string | null
}): string | null {
  // Installer-computed bytes are never an authoritative identity.
  if (!options.githubDigest) return null
  return githubDigestToSha256(options.githubDigest)
}

export function proposeNpmPin(packument: NpmPackument, requested?: string) {
  const version = requested ?? packument["dist-tags"]?.latest
  if (!version || !packument.versions?.[version]?.dist?.integrity) {
    return {
      status: "unavailable" as const,
      reason: "npm registry did not provide version+integrity",
    }
  }
  return {
    status: "supported" as const,
    version,
    integrity: packument.versions[version].dist?.integrity,
    tarball: packument.versions[version].dist?.tarball,
  }
}

export function pypiWheelHashes(files: PypiFile[]): string[] {
  return [
    ...new Set(
      files
        .filter((file) => file.packagetype === "bdist_wheel" && !file.yanked && file.digests?.sha256)
        .map((file) => file.digests!.sha256!)
    ),
  ].sort()
}

function sha256(bytes: Uint8Array | string): string {
  return createHash("sha256").update(bytes).digest("hex")
}

export function contentTreeManifest(
  roots: string[],
  files: ContentTreeFile[],
  artifactSha256?: string
): ContentTreeManifest {
  const sorted = [...files].sort((left, right) =>
    left.path < right.path ? -1 : left.path > right.path ? 1 : 0
  )
  if (new Set(sorted.map((file) => file.path)).size !== sorted.length) {
    throw new Error("duplicate content tree path")
  }
  const tree = createHash("sha256")
  for (const file of sorted) {
    const components = file.path.split("/")
    if (
      !file.path ||
      file.path.startsWith("/") ||
      file.path.includes("\\") ||
      !/^[A-Za-z0-9@._/-]+$/.test(file.path) ||
      components.some((component) => !component || component === "." || component === "..")
    ) {
      throw new Error(`unsafe content tree path: ${file.path}`)
    }
    tree.update(file.path)
    tree.update("\0")
    tree.update(String(file.bytes.byteLength))
    tree.update("\0")
    tree.update(sha256(file.bytes))
    tree.update("\n")
  }
  return {
    version: 1,
    roots: [...roots].sort(),
    fileCount: sorted.length,
    treeSha256: tree.digest("hex"),
    ...(artifactSha256 ? { artifactSha256 } : {}),
  }
}

export function shouldWriteCatalog(argv: string[]): boolean {
  return argv.includes("--write")
}

export function catalogWritePlan(argv: string[], dest: string) {
  return {
    write: shouldWriteCatalog(argv),
    dest,
  }
}

async function fetchJson(url: string): Promise<unknown> {
  const response = await fetch(url, {
    headers: { "User-Agent": "yuzora-lsp-catalog-update", Accept: "application/json" },
  })
  if (!response.ok) {
    throw new Error(`${url} → HTTP ${response.status}`)
  }
  return response.json()
}

export async function buildProposal(fetchImpl: typeof fetchJson = fetchJson) {
  const binaries = []
  for (const route of BINARY_ROUTES) {
    const release = (await fetchImpl(
      `https://api.github.com/repos/${route.repo}/releases/tags/${route.version}`
    )) as GitHubRelease
    binaries.push({
      language: route.language,
      serverId: route.serverId,
      version: route.version,
      platforms: proposeBinaryPlatforms(release, route.assets),
    })
  }
  return {
    catalogVersion: "1",
    note: "Proposal only. Review before replacing src-tauri/lsp-catalog. Do not trust locally hashed download bytes.",
    binaries,
  }
}

export async function writeProposal(dest: string, proposal: unknown) {
  await Bun.write(dest, `${JSON.stringify(proposal, null, 2)}\n`)
}

if (import.meta.main) {
  const proposal = await buildProposal()
  const plan = catalogWritePlan(process.argv.slice(2), "src-tauri/lsp-catalog/proposed-manifest.json")
  if (plan.write) {
    await writeProposal(plan.dest, proposal)
    console.log(`Wrote proposal to ${plan.dest} for review. Catalog was not auto-trusted.`)
  } else {
    console.log(JSON.stringify(proposal, null, 2))
    console.log("\nDry run only. Pass --write to emit a proposal file for review.")
  }
}
