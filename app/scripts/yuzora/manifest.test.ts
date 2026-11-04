import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { archiveName, channelOf, checkVersion, downloadsFor, tauriManifest, yuzoraManifest } from "./manifest";

function dist(files: Record<string, string>) {
  const dir = mkdtempSync(join(tmpdir(), "yuzora-manifest-"));
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text);
  return dir;
}

const when = new Date("2026-11-03T01:02:03.456Z");
const tauriSig = Buffer.from("untrusted comment: signature from tauri secret key\nRUR...\ntrusted comment: timestamp:1\nxx==\n").toString("base64");

describe("yuzora-update.json", () => {
  test("lists every archive with its sha256, the changelog, and the channel of the version", () => {
    const dir = dist({
      [archiveName("0.1.0-beta.3", "macos-aarch64")]: "mac",
      [archiveName("0.1.0-beta.3", "windows-x86_64")]: "win",
      [archiveName("0.1.0-beta.2", "linux-x86_64")]: "an older one",
      "changelog.md": "## [0.1.0-beta.3]",
      "Yuzora-macos-aarch64.dmg": "dmg",
    });
    const doc = yuzoraManifest(dir, "0.1.0-beta.3", downloadsFor("0.1.0-beta.3", undefined), when) as any;
    expect(doc).toMatchObject({ schema: 1, name: "yuzora", version: "0.1.0-beta.3", channel: "beta", pub_date: "2026-11-03T01:02:03Z" });
    expect(Object.keys(doc.targets)).toEqual(["macos-aarch64", "windows-x86_64"]);
    expect(doc.targets["macos-aarch64"]).toEqual({
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.3/yuzora-0.1.0-beta.3-macos-aarch64.tar.gz",
      // sha256("mac")
      sha256: "348a629f5ceed032c3e8706ec47d9bfafb00fb4250b018dd965435ca50cb836e",
      size: 3,
    });
    expect(doc.notes.url).toEndWith("/v0.1.0-beta.3/changelog.md");
    expect(doc.localized_notes).toBeUndefined();
  });

  test("the Chinese changelog goes under localized_notes, by the app's locale", () => {
    const dir = dist({ [archiveName("0.1.0-beta.3", "linux-x86_64")]: "x", "changelog.md": "en", "changelog.zh-TW.md": "zh" });
    const doc = yuzoraManifest(dir, "0.1.0-beta.3") as any;
    expect(Object.keys(doc.localized_notes)).toEqual(["zh-TW"]);
    expect(doc.localized_notes["zh-TW"]).toEqual({
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.3/changelog.zh-TW.md",
      // sha256("zh")
      sha256: "60cae1d01739facc4543771db2e8fbc4d1ebe5fc66284a5b8b4bd1ed585f452e",
      size: 2,
    });
    expect(doc.notes.url).toEndWith("/v0.1.0-beta.3/changelog.md");
  });

  test("stable versions are on stable; odd versions, targets and empty folders fail", () => {
    expect(channelOf("0.1.0")).toBe("stable");
    expect(channelOf("0.1.0-beta.2")).toBe("beta");
    expect(() => checkVersion("0.1")).toThrow();
    expect(() => checkVersion("v0.1.0")).toThrow();
    expect(() => yuzoraManifest(dist({}), "0.1.0")).toThrow("no yuzora-0.1.0-*.tar.gz");
    expect(() => yuzoraManifest(dist({ [archiveName("0.1.0", "macos-x86_64")]: "intel" }), "0.1.0")).toThrow("not a target Yuzora ships");
  });

  test("a test server's downloads", () => {
    const doc = yuzoraManifest(dist({ [archiveName("0.1.0", "linux-x86_64")]: "x" }), "0.1.0", downloadsFor("0.1.0", "http://127.0.0.1:8/R/download/v0.1.0/")) as any;
    expect(doc.targets["linux-x86_64"].url).toBe("http://127.0.0.1:8/R/download/v0.1.0/yuzora-0.1.0-linux-x86_64.tar.gz");
  });
});

describe("latest.json for the Tauri app", () => {
  test("points darwin-aarch64 at the macOS archive and windows-x86_64 at the installer", () => {
    const mac = archiveName("0.1.0-beta.2", "macos-aarch64");
    const dir = dist({ [mac]: "m", [`${mac}.sig`]: tauriSig, "Yuzora-windows-x64-setup.exe": "w", "Yuzora-windows-x64-setup.exe.sig": tauriSig });
    const doc = tauriManifest(dir, "0.1.0-beta.2", "notes", downloadsFor("0.1.0-beta.2", undefined), when);
    expect(doc.version).toBe("0.1.0-beta.2");
    expect(Object.keys(doc.platforms)).toEqual(["darwin-aarch64", "windows-x86_64"]);
    expect(doc.platforms["darwin-aarch64"].url).toBe(`https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.2/${mac}`);
    expect(doc.platforms["windows-x86_64"]).toEqual({ url: "https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.2/Yuzora-windows-x64-setup.exe", signature: tauriSig });
    // Never an Intel Mac entry (AGENTS.md).
    expect(JSON.stringify(doc)).not.toContain("darwin-x86_64");
  });

  test("refuses a half manifest: a missing file or signature, or a plain minisign one", () => {
    const mac = archiveName("0.1.0-beta.2", "macos-aarch64");
    expect(() => tauriManifest(dist({ [mac]: "m", [`${mac}.sig`]: tauriSig }), "0.1.0-beta.2", "n")).toThrow("Yuzora-windows-x64-setup.exe is missing");
    expect(() => tauriManifest(dist({ [mac]: "m", "Yuzora-windows-x64-setup.exe": "w", "Yuzora-windows-x64-setup.exe.sig": tauriSig }), "0.1.0-beta.2", "n")).toThrow(`${mac}.sig is missing`);
    expect(() => tauriManifest(dist({ [mac]: "m", [`${mac}.sig`]: "untrusted comment: x", "Yuzora-windows-x64-setup.exe": "w", "Yuzora-windows-x64-setup.exe.sig": tauriSig }), "0.1.0-beta.2", "n")).toThrow("not a Tauri updater signature");
  });

  test("the Windows installer's old-app prompt matches the current migration status", () => {
    const nsi = readFileSync(join(import.meta.dir, "../../distribution/yuzora/windows/yuzora.nsi"), "utf8");
    const en = nsi.match(/^LangString OldApp \$\{LANG_ENGLISH\} "(.*)"$/m)?.[1] ?? "";
    const zh = nsi.match(/^LangString OldApp \$\{LANG_TRADCHINESE\} "(.*)"$/m)?.[1] ?? "";
    expect(en).toContain("does not delete your old data");
    expect(en).toContain("does not bring connections and settings over automatically yet");
    expect(en).not.toContain("can bring it over");
    expect(zh).toContain("不會刪掉舊版資料");
    expect(zh).toContain("目前還不會自動帶入連線與設定");
    expect(zh).not.toContain("可以把它帶過來");
  });
});
