// yuzora(release): the read-only half of .github/workflows/app-release.yml
// (docs/beta2/release.md, "Releasing"):
//
//   bun scripts/yuzora/release.ts plan
//
// Reads STAGE, VERSION, PACKAGE_RUN_ID and DOWNLOADS_BASE from
// the environment (the workflow passes inputs that way, never inside a script),
// checks them against GitHub (the accepted app-package run, the tag) and
// against the commit being released, and writes what the later jobs need to
// $GITHUB_OUTPUT. It only reads: the jobs that write to the release run plain
// `gh` commands and execute none of this.
import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { REPO, TARGETS, archiveName, checkVersion, unlistedTargets } from "./manifest";

export const STAGES = ["dry-run", "draft", "publish", "manifest", "latest"] as const;
export type Stage = (typeof STAGES)[number];

export const PACKAGE_WORKFLOW = ".github/workflows/app-package.yml";
export const GITHUB_DOWNLOADS = `https://github.com/${REPO}/releases/download`;
/// A dry run signs a manifest that may name only a server on this machine, so
/// what comes out of it can never be accepted by an installed app.
export const LOCAL_DOWNLOADS = /^http:\/\/(?:127\.0\.0\.1|localhost)(?::[0-9]{1,5})?\/[A-Za-z0-9._/-]*$/;
export const DEFAULT_LOCAL_DOWNLOADS = `http://127.0.0.1:8765/${REPO}/releases/download`;

export function isPrerelease(version: string) {
  return version.includes("-");
}

export type Inputs = { stage: Stage; version: string; runId: string; downloads: string };

export function parseInputs(env: Record<string, string | undefined>): Inputs {
  const stage = (env.STAGE ?? "") as Stage;
  if (!STAGES.includes(stage)) throw new Error(`stage must be one of ${STAGES.join(", ")}`);
  const version = checkVersion(env.VERSION ?? "");
  const runId = env.PACKAGE_RUN_ID ?? "";
  if (runId !== "" && !/^[1-9][0-9]{0,19}$/.test(runId)) throw new Error(`package run id must be a number: ${runId}`);
  if (runId === "" && stage !== "dry-run") throw new Error(`stage ${stage} needs the app-package run that was accepted`);
  let downloads = GITHUB_DOWNLOADS;
  if (stage === "dry-run") {
    downloads = (env.DOWNLOADS_BASE || DEFAULT_LOCAL_DOWNLOADS).replace(/\/+$/, "");
    if (!LOCAL_DOWNLOADS.test(downloads)) throw new Error(`a dry run may only name a server on this machine (http://127.0.0.1:<port>/...): ${downloads}`);
  }
  return { stage, version, runId, downloads };
}

/// Why a run cannot be the source of a release, or its commit.
export function checkRun(run: Record<string, any>, repo: string): string {
  const fail = (why: string) => {
    throw new Error(`app-package run ${run.id}: ${why}`);
  };
  if (run.repository?.full_name !== repo || run.head_repository?.full_name !== repo) fail("not a run of this repository's own branch");
  if (String(run.path ?? "").split("@")[0] !== PACKAGE_WORKFLOW) fail(`not a run of ${PACKAGE_WORKFLOW} (${run.path})`);
  // A pull_request run builds GitHub's merge of the branch into its base, not the commit itself.
  if (!["push", "workflow_dispatch"].includes(run.event)) fail(`event ${run.event} builds a merge commit, not the commit that would be tagged`);
  if (run.status !== "completed" || run.conclusion !== "success") fail(`did not succeed (${run.status}/${run.conclusion})`);
  if (!/^[0-9a-f]{40}$/.test(run.head_sha ?? "")) fail("no commit");
  return run.head_sha;
}

/// `tagCommit`: where v<version> points now (null: no such tag).
export function checkTag(stage: Stage, tagCommit: string | null, sha: string, tag: string) {
  if (stage === "dry-run") return;
  if (tagCommit === null) {
    if (stage !== "draft") throw new Error(`${tag} does not exist yet; run the draft stage first`);
    return;
  }
  if (tagCommit !== sha) throw new Error(`${tag} points at ${tagCommit}, not at the accepted commit ${sha}; tags are never moved`);
}

/// What goes to the release before the manifest: the file in the app-package
/// artifacts and the name it gets on the release. A stable release also
/// carries the fixed names the website and README link to.
export function uploadPlan(version: string): { src: string; dest: string }[] {
  const plan = TARGETS.map((t) => ({ src: archiveName(version, t), dest: archiveName(version, t) }));
  const installers: [string, string, string][] = [
    ["Yuzora-macos-aarch64.dmg", `Yuzora_${version}_aarch64.dmg`, "Yuzora-macos-aarch64.dmg"],
    ["Yuzora-windows-x64-setup.exe", `Yuzora_${version}_x64-setup.exe`, "Yuzora-windows-x64-setup.exe"],
  ];
  for (const [src, versioned, fixed] of installers) {
    plan.push({ src, dest: versioned });
    if (!isPrerelease(version)) plan.push({ src, dest: fixed });
  }
  for (const name of ["changelog.md", "changelog.zh-TW.md"]) plan.push({ src: name, dest: name });
  return plan;
}

/// The lines under `## [<version>]` up to the next `## [`.
export function sectionOf(changelog: string, version: string): string {
  const lines = changelog.split("\n");
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start < 0) return "";
  const end = lines.findIndex((l, i) => i > start && l.startsWith("## ["));
  return lines.slice(start + 1, end < 0 ? undefined : end).join("\n").trim();
}

/// The release page text, as beta.2 to beta.4 had it: Chinese first.
export function releaseBody(version: string, zh: string, en: string): string {
  const zhSection = sectionOf(zh, version);
  const enSection = sectionOf(en, version);
  if (!zhSection) throw new Error(`CHANGELOG.zh-TW.md has no section for ${version}`);
  if (!enSection) throw new Error(`CHANGELOG.md has no section for ${version}`);
  return `> 繁體中文在前，English below.\n\n${zhSection}\n\n---\n\n## English\n\n${enSection}\n`;
}

type Api = { status: number; body: any };

/// `gh api`, reading only. 404 comes back as a value, anything else odd throws.
export function api(path: string): Api {
  const out = spawnSync("gh", ["api", "--include", path], { encoding: "utf8" });
  const text = out.stdout ?? "";
  const status = Number(/^HTTP\/\S+ (\d{3})/.exec(text)?.[1] ?? "0");
  if (status === 404) return { status, body: null };
  if (out.status !== 0 || status < 200 || status >= 300) throw new Error(`gh api ${path}: ${(out.stderr || text).trim().split("\n")[0] || `status ${status}`}`);
  const body = text.slice(text.search(/\r?\n\r?\n/)).trim();
  return { status, body: body ? JSON.parse(body) : null };
}

function git(...args: string[]): string {
  const out = spawnSync("git", args, { encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr.trim()}`);
  return out.stdout;
}

/// The commit a tag points at, or null when there is no such tag.
export function tagCommit(tag: string, repo = process.env.GITHUB_REPOSITORY ?? REPO): string | null {
  const ref = api(`repos/${repo}/git/ref/tags/${tag}`);
  if (ref.status === 404) return null;
  let { type, sha } = ref.body.object as { type: string; sha: string };
  if (type === "tag") {
    const annotated = api(`repos/${repo}/git/tags/${sha}`).body.object as { type: string; sha: string };
    ({ type, sha } = annotated);
  }
  if (type !== "commit") throw new Error(`${tag} does not point at a commit`);
  return sha;
}

export function setOutput(file: string | undefined, name: string, value: string) {
  const text = value.includes("\n") ? `${name}<<yuzora_eof\n${value}\nyuzora_eof\n` : `${name}=${value}\n`;
  if (file) appendFileSync(file, text);
  else process.stdout.write(text);
}

export function plan(env: Record<string, string | undefined>) {
  const repo = env.GITHUB_REPOSITORY ?? REPO;
  const inputs = parseInputs(env);
  const { stage, version } = inputs;
  const tag = `v${version}`;
  let sha = env.GITHUB_SHA ?? "";
  if (inputs.runId) {
    const run = api(`repos/${repo}/actions/runs/${inputs.runId}`);
    if (run.status === 404) throw new Error(`no run ${inputs.runId} in ${repo}`);
    sha = checkRun(run.body, repo);
  }
  if (!/^[0-9a-f]{40}$/.test(sha)) throw new Error("no commit to release");
  // The checkout is the workflow's own ref (its tooling), with every branch's history: the commit being released is read from it.
  git("cat-file", "-e", `${sha}^{commit}`);
  const cargo = git("show", `${sha}:app/Cargo.toml`).match(/^version = "(.*)"/m)?.[1] ?? "";
  if (stage !== "dry-run" && cargo !== version) throw new Error(`app/Cargo.toml at ${sha} says ${cargo}, not ${version}`);
  checkTag(stage, stage === "dry-run" ? null : tagCommit(tag, repo), sha, tag);

  const out: Record<string, string> = {
    stage,
    version,
    tag,
    sha,
    run_id: inputs.runId,
    prerelease: String(isPrerelease(version)),
    cargo_version: cargo,
    exclude: unlistedTargets(version).join(","),
    downloads: inputs.downloads,
    pub_date: git("show", "-s", "--format=%cI", sha).trim(),
    upload_plan: uploadPlan(version).map(({ src, dest }) => `${src}\t${dest}`).join("\n"),
    expected_assets: uploadPlan(version).map(({ dest }) => dest).join("\n"),
  };
  if (stage === "draft") {
    // A tag at a commit that is not on main is a release of unmerged code.
    const compare = api(`repos/${repo}/compare/main...${sha}`);
    if (!["behind", "identical"].includes(compare.body?.status)) throw new Error(`${sha} is not on main (${compare.body?.status}); merge it first`);
    if (git("diff", "--name-only", "origin/main", sha, "--", ".github/workflows").trim()) {
      process.stdout.write(`::warning::${sha} differs from main in .github/workflows; GitHub may refuse the token to tag it\n`);
    }
    out.notes = releaseBody(version, git("show", `${sha}:CHANGELOG.zh-TW.md`), git("show", `${sha}:CHANGELOG.md`));
  }
  for (const [name, value] of Object.entries(out)) setOutput(env.GITHUB_OUTPUT, name, value);
  return out;
}

if (import.meta.main) {
  try {
    if (process.argv[2] !== "plan") throw new Error("usage: bun scripts/yuzora/release.ts plan");
    plan(process.env);
  } catch (e) {
    console.error(`::error::${(e as Error).message}`);
    process.exit(1);
  }
}
