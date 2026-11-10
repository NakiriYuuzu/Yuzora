// yuzora(L7): writes a release's update manifests from what
// scripts/yuzora/package left in a folder (docs/beta2/release.md):
//
//   bun scripts/yuzora/manifest.ts yuzora <dist> [--exclude <target>,...] [--pub-date <iso>]
//                                                  -> <dist>/yuzora-update.json
//   bun scripts/yuzora/manifest.ts tauri <dist>    -> <dist>/latest.json
//
// yuzora-update.json is what Yuzora 0.1.0-beta.2 and later read
// (src/yuzora/release/manifest.rs): every update archive with its sha256 and
// size, and changelog.md (English) and changelog.zh-TW.md if they're there.
// The release signs it into
// yuzora-update.json.sig; nothing here signs. --exclude leaves a target out
// of the manifest (its archive still ships with the release): a platform
// whose update would fail is better not offered. --pub-date pins pub_date so a
// release step run twice writes the same bytes.
//
// latest.json is for the Tauri app (0.1.0-beta.1 and older): its updater
// moves people to this release. It needs the Tauri updater signature of the
// macOS archive and of the Windows installer beside them (`<file>.sig`, from
// `tauri signer sign`), and refuses to write a half manifest.
//
// The version comes from Cargo.toml; for a test release server,
// YUZORA_VERSION picks another and YUZORA_DOWNLOADS where files are
// downloaded from (default: this version's GitHub release).
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

export const MANIFEST = "yuzora-update.json";
export const TAURI_MANIFEST = "latest.json";
export const REPO = "NakiriYuuzu/Yuzora";
/// What the app accepts (src/yuzora/release.rs `target()`); the macOS app is Apple Silicon only.
export const TARGETS = ["macos-aarch64", "windows-x86_64", "linux-x86_64", "linux-aarch64"];
/// Targets a version's manifest does not offer, by exact version (a version with no row offers every target). The
/// archive still ships with the release. 0.1.0 leaves Windows out: an installed beta.2 to beta.4 cannot swap itself
/// to it (#261), so the update prompt would never finish; 0.1.1 has no row. The workflow reads this table twice, to
/// write the manifest and to check the signed one; nothing typed at a dispatch can change it.
export const UNLISTED_TARGETS: Record<string, string[]> = {
  "0.1.0": ["windows-x86_64"],
};
export const SCHEMA = 1;
/// The notes in other languages, by the app's i18n locale, beside changelog.md.
export const LOCALIZED_NOTES: Record<string, string> = { "zh-TW": "changelog.zh-TW.md" };

export function checkVersion(version: string) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?$/.test(version)) throw new Error(`not a release version: ${version}`);
  return version;
}

export function unlistedTargets(version: string): string[] {
  const unlisted = UNLISTED_TARGETS[version] ?? [];
  for (const target of unlisted) if (!TARGETS.includes(target)) throw new Error(`UNLISTED_TARGETS[${version}]: ${target} is not a target Yuzora ships`);
  if (new Set(unlisted).size !== unlisted.length) throw new Error(`UNLISTED_TARGETS[${version}] names a target twice`);
  return unlisted;
}

export function listedTargets(version: string): string[] {
  const unlisted = unlistedTargets(version);
  return TARGETS.filter((t) => !unlisted.includes(t));
}

export function channelOf(version: string) {
  return version.includes("-") ? "beta" : "stable";
}

export function downloadsFor(version: string, base = process.env.YUZORA_DOWNLOADS) {
  return (base ?? `https://github.com/${REPO}/releases/download/v${version}`).replace(/\/+$/, "");
}

function sha256(file: string) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

type Asset = { url: string; sha256: string; size: number };

function asset(dist: string, name: string, downloads: string): Asset {
  const file = join(dist, name);
  return { url: `${downloads}/${name}`, sha256: sha256(file), size: statSync(file).size };
}

export function archiveName(version: string, target: string) {
  return `yuzora-${version}-${target}.tar.gz`;
}

/// The manifest Yuzora reads: one entry per archive found, at least one,
/// except the targets in `exclude`.
export function yuzoraManifest(dist: string, version: string, downloads = downloadsFor(version), date = new Date(), exclude: string[] = []) {
  checkVersion(version);
  for (const target of exclude) if (!TARGETS.includes(target)) throw new Error(`cannot leave out ${target}: not a target Yuzora ships`);
  const targets: Record<string, Asset> = {};
  const archive = new RegExp(`^yuzora-${version.replace(/\./g, "\\.")}-(.+)\\.tar\\.gz$`);
  for (const name of readdirSync(dist).sort()) {
    const m = name.match(archive);
    if (!m) continue;
    if (!TARGETS.includes(m[1])) throw new Error(`${name}: ${m[1]} is not a target Yuzora ships`);
    if (exclude.includes(m[1])) continue;
    targets[m[1]] = asset(dist, name, downloads);
  }
  if (Object.keys(targets).length === 0) throw new Error(`no yuzora-${version}-*.tar.gz in ${dist}${exclude.length > 0 ? ` besides the excluded ${exclude.join(", ")}` : ""}`);
  const doc: Record<string, unknown> = { schema: SCHEMA, name: "yuzora", version, channel: channelOf(version), pub_date: date.toISOString().replace(/\.\d+Z$/, "Z") };
  if (existsSync(join(dist, "changelog.md"))) doc.notes = asset(dist, "changelog.md", downloads);
  const localized: Record<string, Asset> = {};
  for (const [locale, name] of Object.entries(LOCALIZED_NOTES)) if (existsSync(join(dist, name))) localized[locale] = asset(dist, name, downloads);
  if (Object.keys(localized).length > 0) doc.localized_notes = localized;
  doc.targets = targets;
  return doc;
}

/// The Tauri updater's latest.json pointing the old app at this release.
export function tauriManifest(dist: string, version: string, notes: string, downloads = downloadsFor(version), date = new Date()) {
  checkVersion(version);
  const platform = (name: string) => {
    const file = join(dist, name);
    if (!existsSync(file)) throw new Error(`${name} is missing`);
    if (!existsSync(`${file}.sig`)) throw new Error(`${name}.sig is missing (tauri signer sign)`);
    const signature = readFileSync(`${file}.sig`, "utf8").trim();
    // A Tauri .sig is the minisign signature file, base64-encoded.
    if (!Buffer.from(signature, "base64").toString("utf8").startsWith("untrusted comment:")) throw new Error(`${name}.sig is not a Tauri updater signature`);
    return { url: `${downloads}/${name}`, signature };
  };
  return {
    version,
    notes,
    pub_date: date.toISOString().replace(/\.\d+Z$/, "Z"),
    platforms: {
      // The Tauri updater swaps the whole bundle and starts the new
      // CFBundleExecutable; the archive's top folder is dropped.
      "darwin-aarch64": platform(archiveName(version, "macos-aarch64")),
      // Run as an NSIS installer (/P /UPDATE /R): distribution/yuzora/windows/yuzora.nsi.
      "windows-x86_64": platform("Yuzora-windows-x64-setup.exe"),
    },
  };
}

export function parseCli(argv: string[]) {
  const { values, positionals } = parseArgs({ args: argv, allowPositionals: true, options: { exclude: { type: "string", default: "" }, "pub-date": { type: "string" } } });
  const [kind, dir] = positionals;
  if (!kind || !dir || positionals.length > 2 || !["yuzora", "tauri"].includes(kind)) throw new Error("usage: bun scripts/yuzora/manifest.ts yuzora|tauri <dist> [--exclude <target>,...] [--pub-date <iso>]");
  const exclude = values.exclude.split(",").map((t) => t.trim()).filter(Boolean);
  if (kind === "tauri" && exclude.length > 0) throw new Error("--exclude is for the yuzora manifest; latest.json always lists the two platforms the Tauri app updates");
  const date = values["pub-date"] === undefined ? new Date() : new Date(values["pub-date"]);
  if (Number.isNaN(date.getTime())) throw new Error(`--pub-date ${values["pub-date"]} is not a date`);
  return { kind, dir, exclude, date };
}

if (import.meta.main) {
  let cli: ReturnType<typeof parseCli>;
  try {
    cli = parseCli(process.argv.slice(2));
  } catch (e) {
    console.error((e as Error).message);
    process.exit(2);
  }
  const { kind, dir, exclude, date } = cli;
  const root = resolve(import.meta.dir, "../..");
  const cargo = readFileSync(join(root, "Cargo.toml"), "utf8");
  const version = checkVersion(process.env.YUZORA_VERSION || (cargo.match(/^version = "(.*)"/m)?.[1] ?? ""));
  const dist = resolve(dir);
  const doc = kind === "yuzora" ? yuzoraManifest(dist, version, downloadsFor(version), date, exclude) : tauriManifest(dist, version, `Yuzora ${version}: https://github.com/${REPO}/releases/tag/v${version}`, downloadsFor(version), date);
  const out = join(dist, kind === "yuzora" ? MANIFEST : TAURI_MANIFEST);
  writeFileSync(out, JSON.stringify(doc, null, 2) + "\n");
  console.log(`manifest: ${out}`);
  console.log(JSON.stringify(doc, null, 2));
}
