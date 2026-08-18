import { readFileSync } from "node:fs"
import { mkdir, writeFile } from "node:fs/promises"
import { dirname, join } from "node:path"

import { finalizeUpdaterMetadata, type UpdaterMetadata } from "./finalize-updater-metadata"

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

function exactlyOne(assetNames: readonly string[], predicate: (name: string) => boolean, label: string) {
  const matches = assetNames.filter(predicate)
  assert(matches.length === 1, `expected exactly one ${label}, found ${matches.length}`)
  return matches[0]!
}

function encodedReleaseAssetUrl(repository: string, tag: string, asset: string) {
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(asset)}`
}

function readSignature(directory: string, asset: string) {
  const signature = readFileSync(join(directory, `${asset}.sig`), "utf8").trim()
  assert(signature.length > 0, `updater signature for ${asset} is empty`)
  return signature
}

export function prepareUpdaterMetadata(
  tag: string,
  repository: string,
  notes: string,
  assetNames: readonly string[],
  signatureDirectory: string,
  publishedAt = new Date().toISOString()
): UpdaterMetadata {
  assert(tag.startsWith("v") && tag.length > 1, "tag must begin with v")
  const version = tag.slice(1)
  assert(repository.includes("/"), "repository must be owner/name")
  assert(notes.trim().length > 0, "updater notes are required")
  assert(!Number.isNaN(Date.parse(publishedAt)), "publishedAt must be an ISO timestamp")

  const versionPrefix = `Yuzora_${version}_`
  const archive = exactlyOne(
    assetNames,
    (name) => name.startsWith(versionPrefix) && name.endsWith(".app.tar.gz"),
    "macOS updater archive"
  )
  const msi = exactlyOne(
    assetNames,
    (name) => name.startsWith(versionPrefix) && name.endsWith(".msi"),
    "Windows MSI updater artifact"
  )
  for (const asset of [archive, msi]) {
    assert(assetNames.includes(`${asset}.sig`), `release is missing updater signature ${asset}.sig`)
  }

  const raw = {
    version,
    notes,
    pub_date: publishedAt,
    platforms: {
      "darwin-aarch64": {
        url: encodedReleaseAssetUrl(repository, tag, archive),
        signature: readSignature(signatureDirectory, archive),
      },
      "darwin-x86_64": {
        url: encodedReleaseAssetUrl(repository, tag, archive),
        signature: readSignature(signatureDirectory, archive),
      },
      "windows-x86_64": {
        url: encodedReleaseAssetUrl(repository, tag, msi),
        signature: readSignature(signatureDirectory, msi),
      },
    },
  }

  return finalizeUpdaterMetadata(raw, assetNames, version)
}

async function main() {
  const [tag, repository, notesPath, assetNamesPath, signatureDirectory, outputPath] = process.argv.slice(2)
  assert(
    tag && repository && notesPath && assetNamesPath && signatureDirectory && outputPath,
    "usage: prepare-updater-metadata.ts <tag> <owner/repo> <notes> <asset-names> <signature-dir> <output>"
  )
  const notes = await Bun.file(notesPath).text()
  const assetNames = (await Bun.file(assetNamesPath).text())
    .split(/\r?\n/)
    .map((name) => name.trim())
    .filter(Boolean)
  const metadata = prepareUpdaterMetadata(tag, repository, notes, assetNames, signatureDirectory)
  await mkdir(dirname(outputPath), { recursive: true })
  await writeFile(outputPath, `${JSON.stringify(metadata, null, 2)}\n`)
  console.log(`Stable updater metadata prepared for ${Object.keys(metadata.platforms).length} platforms`)
}

if (import.meta.main) {
  main().catch((error) => {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  })
}
