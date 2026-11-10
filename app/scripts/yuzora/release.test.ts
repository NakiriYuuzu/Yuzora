import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { DEFAULT_LOCAL_DOWNLOADS, GITHUB_DOWNLOADS, api, checkRun, checkTag, parseInputs, plan, releaseBody, sectionOf, uploadPlan } from "./release";

const SHA = "a".repeat(40);
const OTHER = "b".repeat(40);
const good = { STAGE: "draft", VERSION: "0.1.0", PACKAGE_RUN_ID: "42" };

describe("inputs", () => {
  test("a stable release with the accepted run", () => {
    expect(parseInputs(good)).toEqual({ stage: "draft", version: "0.1.0", runId: "42", downloads: GITHUB_DOWNLOADS });
  });

  test("anything but a known stage, a plain version, or a numeric run id is refused", () => {
    expect(() => parseInputs({ ...good, STAGE: "ship" })).toThrow("stage must be one of");
    expect(() => parseInputs({ ...good, VERSION: "v0.1.0" })).toThrow("not a release version");
    expect(() => parseInputs({ ...good, VERSION: "0.1.0; rm -rf /" })).toThrow("not a release version");
    expect(() => parseInputs({ ...good, PACKAGE_RUN_ID: "4 2" })).toThrow("must be a number");
    expect(() => parseInputs({ ...good, PACKAGE_RUN_ID: "$(id)" })).toThrow("must be a number");
    expect(() => parseInputs({ ...good, PACKAGE_RUN_ID: "" })).toThrow("needs the app-package run");
  });

  test("only a dry run may go without a run, and it names only a server on this machine", () => {
    const dry = parseInputs({ STAGE: "dry-run", VERSION: "0.1.0" });
    expect(dry.runId).toBe("");
    expect(dry.downloads).toBe(DEFAULT_LOCAL_DOWNLOADS);
    expect(parseInputs({ STAGE: "dry-run", VERSION: "0.1.0", DOWNLOADS_BASE: "http://localhost:9/NakiriYuuzu/Yuzora/releases/download/" }).downloads).toBe("http://localhost:9/NakiriYuuzu/Yuzora/releases/download");
    for (const base of [GITHUB_DOWNLOADS, "http://127.0.0.1.evil.example/x", "https://127.0.0.1/x", "http://192.168.1.2:80/x", "http://127.0.0.1:8765"]) {
      expect(() => parseInputs({ STAGE: "dry-run", VERSION: "0.1.0", DOWNLOADS_BASE: base })).toThrow("only name a server on this machine");
    }
    // The other stages ignore the field: a release is always downloaded from GitHub.
    expect(parseInputs({ ...good, DOWNLOADS_BASE: "http://127.0.0.1:1/x" }).downloads).toBe(GITHUB_DOWNLOADS);
  });

  test("no input says which targets the manifest leaves out: that is the version's row in manifest.ts", () => {
    expect(parseInputs({ ...good, EXCLUDE_TARGETS: "linux-aarch64" })).toEqual(parseInputs(good));
  });
});

describe("the accepted run and the tag", () => {
  const run = { id: 42, repository: { full_name: "NakiriYuuzu/Yuzora" }, head_repository: { full_name: "NakiriYuuzu/Yuzora" }, path: ".github/workflows/app-package.yml", event: "push", status: "completed", conclusion: "success", head_sha: SHA };

  test("a finished, successful package run of a branch of this repository gives its commit", () => {
    expect(checkRun(run, "NakiriYuuzu/Yuzora")).toBe(SHA);
    expect(checkRun({ ...run, event: "workflow_dispatch" }, "NakiriYuuzu/Yuzora")).toBe(SHA);
  });

  test.each([
    ["a fork", { head_repository: { full_name: "someone/Yuzora" } }, "own branch"],
    ["another workflow", { path: ".github/workflows/app-ci.yml" }, "not a run of"],
    ["a pull request, which builds the merge commit", { event: "pull_request" }, "merge commit"],
    ["a failure", { conclusion: "failure" }, "did not succeed"],
    ["a run still going", { status: "in_progress", conclusion: null }, "did not succeed"],
    ["no commit", { head_sha: "" }, "no commit"],
  ])("%s is refused", (_name, change, why) => {
    expect(() => checkRun({ ...run, ...change }, "NakiriYuuzu/Yuzora")).toThrow(why);
  });

  test("a tag is created by the draft stage, and never moved", () => {
    expect(() => checkTag("draft", null, SHA, "v0.1.0")).not.toThrow();
    expect(() => checkTag("draft", SHA, SHA, "v0.1.0")).not.toThrow();
    expect(() => checkTag("draft", OTHER, SHA, "v0.1.0")).toThrow("never moved");
    for (const stage of ["publish", "manifest", "latest"] as const) {
      expect(() => checkTag(stage, SHA, SHA, "v0.1.0")).not.toThrow();
      expect(() => checkTag(stage, null, SHA, "v0.1.0")).toThrow("run the draft stage first");
      expect(() => checkTag(stage, OTHER, SHA, "v0.1.0")).toThrow("never moved");
    }
  });
});

describe("what a release carries", () => {
  test("a stable release has the fixed names beside the versioned ones", () => {
    const plan = uploadPlan("0.1.0");
    expect(plan.map((p) => p.dest)).toEqual([
      "yuzora-0.1.0-macos-aarch64.tar.gz",
      "yuzora-0.1.0-windows-x86_64.tar.gz",
      "yuzora-0.1.0-linux-x86_64.tar.gz",
      "yuzora-0.1.0-linux-aarch64.tar.gz",
      "Yuzora_0.1.0_aarch64.dmg",
      "Yuzora-macos-aarch64.dmg",
      "Yuzora_0.1.0_x64-setup.exe",
      "Yuzora-windows-x64-setup.exe",
      "changelog.md",
      "changelog.zh-TW.md",
    ]);
    expect(plan.filter((p) => p.dest.startsWith("Yuzora")).map((p) => p.src)).toEqual(["Yuzora-macos-aarch64.dmg", "Yuzora-macos-aarch64.dmg", "Yuzora-windows-x64-setup.exe", "Yuzora-windows-x64-setup.exe"]);
  });

  test("a pre-release has no fixed names (what beta.2 to beta.4 shipped)", () => {
    const names = uploadPlan("0.1.0-beta.4").map((p) => p.dest);
    expect(names).toContain("Yuzora_0.1.0-beta.4_aarch64.dmg");
    expect(names).toContain("Yuzora_0.1.0-beta.4_x64-setup.exe");
    expect(names).not.toContain("Yuzora-macos-aarch64.dmg");
    expect(names).not.toContain("Yuzora-windows-x64-setup.exe");
    expect(names).toHaveLength(8);
  });

  test("the page text puts the Chinese notes first, from the version's own section", () => {
    const en = "# Changelog\n\n## [0.1.0] - 2026-11-12\n\nEnglish one.\n\n### Added\n\n- a\n\n## [0.1.0-beta.4] - 2026-11-10\n\nOld.\n";
    const zh = "# Changelog\n\n## [0.1.0] - 2026-11-12\n\n中文一。\n\n### 新增\n\n- 甲\n\n## [0.1.0-beta.4] - 2026-11-10\n\n舊的。\n";
    expect(sectionOf(en, "0.1.0")).toBe("English one.\n\n### Added\n\n- a");
    expect(sectionOf(en, "0.1.0-beta.4")).toBe("Old.");
    expect(sectionOf(en, "9.9.9")).toBe("");
    expect(releaseBody("0.1.0", zh, en)).toBe("> 繁體中文在前，English below.\n\n中文一。\n\n### 新增\n\n- 甲\n\n---\n\n## English\n\nEnglish one.\n\n### Added\n\n- a\n");
    expect(() => releaseBody("0.1.1", zh, en)).toThrow("CHANGELOG.zh-TW.md has no section for 0.1.1");
    expect(() => releaseBody("0.1.0", zh, "# Changelog\n")).toThrow("CHANGELOG.md has no section for 0.1.0");
  });
});

describe("plan, against a fake gh and a throwaway repository", () => {
  let work: string;
  let fixtures: string;
  let output: string;
  let sha: string;
  let previousCwd: string;
  let previousPath: string | undefined;

  const git = (...args: string[]) => {
    const r = spawnSync("git", args, { cwd: work, encoding: "utf8" });
    if (r.status !== 0) throw new Error(r.stderr);
    return r.stdout.trim();
  };
  const reply = (path: string, status: number, body: unknown) => writeFileSync(join(fixtures, path.replace(/[/?=&]/g, "_")), `HTTP/2.0 ${status} X\r\n\r\n${JSON.stringify(body)}`);

  beforeEach(() => {
    previousCwd = process.cwd();
    previousPath = process.env.PATH;
    work = mkdtempSync(join(tmpdir(), "yuzora-plan-"));
    fixtures = join(work, "..", `${work.split("/").pop()}-fixtures`);
    mkdirSync(fixtures, { recursive: true });
    output = join(fixtures, "out");
    writeFileSync(output, "");
    const bin = join(fixtures, "bin");
    mkdirSync(bin, { recursive: true });
    // gh api --include <path>: the canned answer for that path, else 404.
    writeFileSync(join(bin, "gh"), `#!/bin/sh\nf="${fixtures}/$(printf %s "$3" | tr '/?=&' '____')"\nif [ -f "$f" ]; then cat "$f"; else printf 'HTTP/2.0 404 Not Found\\r\\n\\r\\n{"message":"Not Found"}'; exit 1; fi\n`);
    chmodSync(join(bin, "gh"), 0o755);
    process.env.PATH = `${bin}:${previousPath}`;
    git("init", "-q", "-b", "main");
    git("config", "user.email", "t@example.test");
    git("config", "user.name", "t");
    mkdirSync(join(work, "app"));
    mkdirSync(join(work, ".github/workflows"), { recursive: true });
    writeFileSync(join(work, "app/Cargo.toml"), '[package]\nname = "herdr-app"\nversion = "0.1.0"\n');
    writeFileSync(join(work, "CHANGELOG.md"), "## [0.1.0] - 2026-11-12\n\nEnglish.\n");
    writeFileSync(join(work, "CHANGELOG.zh-TW.md"), "## [0.1.0] - 2026-11-12\n\n中文。\n");
    writeFileSync(join(work, ".github/workflows/x.yml"), "name: x\n");
    git("add", "-A");
    git("commit", "-q", "-m", "release prep");
    sha = git("rev-parse", "HEAD");
    git("update-ref", "refs/remotes/origin/main", sha);
    process.chdir(work);
  });
  afterEach(() => {
    process.chdir(previousCwd);
    process.env.PATH = previousPath;
  });

  const env = (extra: Record<string, string> = {}) => ({ STAGE: "draft", VERSION: "0.1.0", PACKAGE_RUN_ID: "42", GITHUB_REPOSITORY: "NakiriYuuzu/Yuzora", GITHUB_OUTPUT: output, GITHUB_SHA: sha, ...extra });
  const runBody = () => ({ id: 42, repository: { full_name: "NakiriYuuzu/Yuzora" }, head_repository: { full_name: "NakiriYuuzu/Yuzora" }, path: ".github/workflows/app-package.yml", event: "push", status: "completed", conclusion: "success", head_sha: sha });
  const accepted = () => {
    reply("repos/NakiriYuuzu/Yuzora/actions/runs/42", 200, runBody());
    reply(`repos/NakiriYuuzu/Yuzora/compare/main...${sha}`, 200, { status: "behind" });
  };

  test("draft: a run on a commit already on main, no tag yet", () => {
    accepted();
    const out = plan(env());
    expect(out).toMatchObject({ stage: "draft", version: "0.1.0", tag: "v0.1.0", sha, run_id: "42", prerelease: "false", exclude: "windows-x86_64", cargo_version: "0.1.0", downloads: GITHUB_DOWNLOADS });
    expect(out.notes).toBe("> 繁體中文在前，English below.\n\n中文。\n\n---\n\n## English\n\nEnglish.\n");
    expect(out.upload_plan.split("\n")).toHaveLength(10);
    expect(out.upload_plan).toContain("Yuzora-macos-aarch64.dmg\tYuzora_0.1.0_aarch64.dmg");
    const written = readFileSync(output, "utf8");
    expect(written).toContain(`sha=${sha}\n`);
    expect(written).toContain("run_id=42\n");
    expect(written).toContain("notes<<yuzora_eof\n> 繁體中文在前");
  });

  test("every output the workflow reads from the plan step is one the plan writes", () => {
    accepted();
    const written = Object.keys(plan(env()));
    const workflow = readFileSync(join(import.meta.dir, "../../../.github/workflows/app-release.yml"), "utf8");
    const read = [...new Set([...workflow.matchAll(/\bplan\.outputs\.([a-z_]+)/g)].map((m) => m[1]))];
    expect(read.length).toBeGreaterThan(5);
    expect(read.filter((name) => !written.includes(name))).toEqual([]);
  });

  test("a tag of the same commit lets the draft stage run again; another commit stops it", () => {
    accepted();
    reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "commit", sha } });
    expect(plan(env()).sha).toBe(sha);
    reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "commit", sha: OTHER } });
    expect(() => plan(env())).toThrow("never moved");
    reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "tag", sha: "c".repeat(40) } });
    reply(`repos/NakiriYuuzu/Yuzora/git/tags/${"c".repeat(40)}`, 200, { object: { type: "commit", sha } });
    expect(plan(env()).sha).toBe(sha);
  });

  test("later stages need the tag", () => {
    accepted();
    expect(() => plan(env({ STAGE: "publish" }))).toThrow("run the draft stage first");
    reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "commit", sha } });
    const out = plan(env({ STAGE: "manifest" }));
    expect(out).toMatchObject({ stage: "manifest", exclude: "windows-x86_64" });
    expect(out.notes).toBeUndefined();
  });

  test("0.1.0 never lists Windows, whatever the dispatch says and at whichever stage", () => {
    accepted();
    reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "commit", sha } });
    for (const stage of ["draft", "publish", "manifest", "latest"]) {
      for (const EXCLUDE_TARGETS of [undefined, "", "linux-aarch64", "windows-x86_64"]) {
        const out = plan(env({ STAGE: stage, ...(EXCLUDE_TARGETS === undefined ? {} : { EXCLUDE_TARGETS }) }));
        expect(out.exclude, `${stage} with ${EXCLUDE_TARGETS}`).toBe("windows-x86_64");
      }
    }
    const dry = plan(env({ STAGE: "dry-run", PACKAGE_RUN_ID: "", GITHUB_SHA: sha }));
    expect(dry.exclude).toBe("windows-x86_64");
  });

  test("a version with no row leaves nothing out", () => {
    const out = plan(env({ STAGE: "dry-run", PACKAGE_RUN_ID: "", VERSION: "0.2.0", GITHUB_SHA: sha }));
    expect(out.exclude).toBe("");
  });

  test("a commit that is not on main, a version Cargo.toml does not have, or a missing run are refused", () => {
    accepted();
    reply(`repos/NakiriYuuzu/Yuzora/compare/main...${sha}`, 200, { status: "ahead" });
    expect(() => plan(env())).toThrow("is not on main");
    accepted();
    expect(() => plan(env({ VERSION: "0.1.1" }))).toThrow("app/Cargo.toml");
    expect(() => plan(env({ PACKAGE_RUN_ID: "43" }))).toThrow("no run 43");
  });

  describe("a published release the dispatch is not about", () => {
    // beta.4 is public, with its package run, its tag and a Cargo.toml that all agree. The workflow runs from a ref that is at 0.1.0.
    const cargo = (version: string) => writeFileSync(join(work, "app/Cargo.toml"), `[package]\nname = "herdr-app"\nversion = "${version}"\n`);
    let beta4: string;
    beforeEach(() => {
      cargo("0.1.0-beta.4");
      git("commit", "-q", "-am", "beta.4");
      beta4 = git("rev-parse", "HEAD");
      cargo("0.1.0");
      git("commit", "-q", "-am", "release prep");
      sha = git("rev-parse", "HEAD");
      git("update-ref", "refs/remotes/origin/main", sha);
      reply("repos/NakiriYuuzu/Yuzora/actions/runs/42", 200, { ...runBody(), head_sha: beta4 });
      reply(`repos/NakiriYuuzu/Yuzora/compare/main...${beta4}`, 200, { status: "behind" });
      reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0-beta.4", 200, { object: { type: "commit", sha: beta4 } });
    });

    test("no stage but the dry run touches it", () => {
      for (const STAGE of ["draft", "publish", "manifest", "latest"]) {
        expect(() => plan(env({ STAGE, VERSION: "0.1.0-beta.4" })), STAGE).toThrow("is not the release this workflow runs from");
      }
      expect(plan(env({ STAGE: "dry-run", VERSION: "0.1.0-beta.4" })).stage).toBe("dry-run");
    });

    test("the release the ref is at goes through, from a later commit than the one released", () => {
      reply("repos/NakiriYuuzu/Yuzora/actions/runs/42", 200, { ...runBody(), head_sha: sha });
      reply(`repos/NakiriYuuzu/Yuzora/compare/main...${sha}`, 200, { status: "identical" });
      reply("repos/NakiriYuuzu/Yuzora/git/ref/tags/v0.1.0", 200, { object: { type: "commit", sha } });
      expect(plan(env({ STAGE: "manifest" })).version).toBe("0.1.0");
      writeFileSync(join(work, "later.txt"), "a site change\n");
      git("add", "later.txt");
      git("commit", "-q", "-m", "site");
      expect(plan(env({ STAGE: "latest", GITHUB_SHA: git("rev-parse", "HEAD") })).version).toBe("0.1.0");
    });
  });

  test("a dry run with no run signs for the commit it runs on and needs no tag or main", () => {
    const out = plan(env({ STAGE: "dry-run", PACKAGE_RUN_ID: "", VERSION: "0.2.0", GITHUB_SHA: sha }));
    expect(out).toMatchObject({ stage: "dry-run", version: "0.2.0", sha, run_id: "", cargo_version: "0.1.0", downloads: DEFAULT_LOCAL_DOWNLOADS });
    expect(readFileSync(output, "utf8")).toContain("run_id=\n");
    expect(out.notes).toBeUndefined();
  });

  test("GitHub's answer can be bigger than a megabyte (compare lists every commit and file of a long branch)", () => {
    accepted();
    reply(`repos/NakiriYuuzu/Yuzora/compare/main...${sha}`, 200, { status: "behind", files: ["x".repeat(3_000_000)] });
    expect(plan(env()).sha).toBe(sha);
  });

  test("when gh cannot answer at all, the error says why instead of an HTTP status line", () => {
    process.env.PATH = join(fixtures, "no-gh");
    expect(() => api("repos/NakiriYuuzu/Yuzora/compare/main...abc")).toThrow(/ENOENT|not found/);
  });

  test("a changed workflow file is only a warning", () => {
    accepted();
    writeFileSync(join(work, ".github/workflows/x.yml"), "name: y\n");
    git("commit", "-q", "-am", "ci change");
    git("update-ref", "refs/remotes/origin/main", git("rev-parse", "HEAD"));
    const lines: string[] = [];
    const write = process.stdout.write.bind(process.stdout);
    process.stdout.write = ((chunk: string) => (lines.push(String(chunk)), true)) as typeof process.stdout.write;
    try {
      expect(plan(env()).sha).toBe(sha);
    } finally {
      process.stdout.write = write;
    }
    expect(lines.join("")).toContain("::warning::");
  });
});
