type UnknownRecord = Record<string, unknown>

export interface UpdaterPlatform {
  url: string
  signature: string
  [key: string]: unknown
}

export interface UpdaterMetadata {
  version: string
  notes: string
  platforms: Record<string, UpdaterPlatform>
  [key: string]: unknown
}

function record(value: unknown, label: string): UnknownRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as UnknownRecord
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

function assetName(url: string): string {
  const name = new URL(url).pathname.split("/").at(-1)
  assert(name, `updater URL has no asset name: ${url}`)
  return decodeURIComponent(name)
}

export function finalizeUpdaterMetadata(
  value: unknown,
  releaseAssetNames: Iterable<string>,
  expectedVersion: string
): UpdaterMetadata {
  const metadata = record(value, "updater metadata")
  const rawPlatforms = record(metadata.platforms, "updater metadata platforms")
  assert(metadata.version === expectedVersion, `metadata version must be ${expectedVersion}`)
  assert(
    typeof metadata.notes === "string" && metadata.notes.trim().length > 0,
    "updater notes are required"
  )

  const platforms: Record<string, UpdaterPlatform> = {}
  for (const [key, rawPlatform] of Object.entries(rawPlatforms)) {
    if (key.startsWith("windows-") && key.endsWith("-nsis")) continue

    const platform = record(rawPlatform, `platform ${key}`)
    assert(typeof platform.url === "string" && platform.url.length > 0, `${key} URL is required`)
    assert(
      typeof platform.signature === "string" && platform.signature.length > 0,
      `${key} signature is required`
    )
    if (key.startsWith("windows-")) {
      assert(platform.url.toLowerCase().endsWith(".msi"), `${key} must point to an MSI asset`)
    }
    platforms[key] = platform as UpdaterPlatform
  }

  assert(platforms["windows-x86_64"], "windows-x86_64 MSI metadata is required")

  const assetNames = new Set(releaseAssetNames)
  for (const platform of Object.values(platforms)) {
    const artifact = assetName(platform.url)
    assert(assetNames.has(artifact), `missing updater asset ${artifact}`)
    assert(assetNames.has(`${artifact}.sig`), `missing signature asset ${artifact}.sig`)
  }

  return {
    ...metadata,
    version: expectedVersion,
    notes: metadata.notes,
    platforms,
  }
}

async function fetchReleaseAssetNames(repository: string, tag: string, token: string) {
  const response = await fetch(`https://api.github.com/repos/${repository}/releases?per_page=100`, {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "User-Agent": "yuzora-updater-release-finalizer",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  })
  if (!response.ok) {
    throw new Error(`GitHub release lookup failed with HTTP ${response.status}`)
  }

  const releases = await response.json()
  assert(Array.isArray(releases), "GitHub releases response must be an array")
  const releaseValue = releases.find((candidate) => {
    const item = record(candidate, "GitHub release candidate")
    return item.tag_name === tag
  })
  assert(releaseValue, `GitHub release ${tag} was not found`)
  const release = record(releaseValue, "GitHub release")
  assert(Array.isArray(release.assets), "GitHub release assets are required")
  return release.assets.map((asset, index) => {
    const item = record(asset, `GitHub release asset ${index}`)
    assert(typeof item.name === "string" && item.name.length > 0, `asset ${index} name is required`)
    return item.name
  })
}

if (import.meta.main) {
  const metadataPath = process.argv[2]
  const repository = process.env.GITHUB_REPOSITORY
  const tag = process.env.GITHUB_REF_NAME
  const token = process.env.GITHUB_TOKEN

  assert(metadataPath, "usage: bun scripts/finalize-updater-metadata.ts <latest.json>")
  assert(repository, "GITHUB_REPOSITORY is required")
  assert(tag, "GITHUB_REF_NAME is required")
  assert(token, "GITHUB_TOKEN is required")

  const expectedVersion = tag.startsWith("v") ? tag.slice(1) : tag
  const metadata = await Bun.file(metadataPath).json()
  const assets = await fetchReleaseAssetNames(repository, tag, token)
  const finalized = finalizeUpdaterMetadata(metadata, assets, expectedVersion)

  await Bun.write(metadataPath, `${JSON.stringify(finalized, null, 2)}\n`)
  console.log(
    `Updater metadata finalized: ${Object.keys(finalized.platforms).length} signed MSI-only platform entries`
  )
}
