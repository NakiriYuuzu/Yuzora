import { afterAll, describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const WATCH = join(import.meta.dir, "sync-watch");
const tmp = mkdtempSync(join(tmpdir(), "sync-watch-"));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

// Git as the test's own person, with nobody's configuration.
const GIT = { GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" };
function run(cwd: string, cmd: string[], env: Record<string, string> = {}) {
  const done = Bun.spawnSync(cmd, { cwd, env: { PATH: process.env.PATH!, ...GIT, ...env } });
  return { status: done.exitCode, out: done.stdout.toString(), err: done.stderr.toString() };
}
function git(cwd: string, ...args: string[]) {
  const done = run(cwd, ["git", ...args]);
  if (done.status !== 0) throw new Error(`git ${args.join(" ")}: ${done.err}`);
  return done.out.trim();
}
function repo(name: string) {
  const dir = join(tmp, name);
  git(tmp, "init", "-q", "-b", "main", dir);
  return dir;
}
const values = (out: string) => Object.fromEntries(out.trim().split("\n").map((line) => line.split(/=(.*)/s).slice(0, 2)));

describe("the newest upstream release", () => {
  const upstream = repo("upstream");
  const commit = (message: string) => (git(upstream, "commit", "-q", "--allow-empty", "-m", message), git(upstream, "rev-parse", "HEAD"));
  commit("one");
  git(upstream, "tag", "v0.1.9");
  const ten = commit("two");
  git(upstream, "tag", "-a", "v0.1.10", "-m", "a tag with a message");
  const three = commit("three");
  git(upstream, "tag", "v0.2.0-rc.1");
  git(upstream, "tag", "nightly");
  commit("four");

  const ours = repo("ours");
  const squash = (split: string) => git(ours, "commit", "-q", "--allow-empty", "-m", `Squashed 'app/' changes\n\ngit-subtree-dir: app\ngit-subtree-split: ${split}`);

  test("is the highest vX.Y.Z by number, at its commit: 0.1.10 is after 0.1.9, and a release candidate is not a release", () => {
    squash("a".repeat(40));
    const found = run(ours, [WATCH], { SYNC_URL: upstream });
    expect(values(found.out)).toEqual({ tag: "v0.1.10", commit: ten, have: "a".repeat(40), new: "true" });
  });

  test("is not new once app/ has its commit", () => {
    squash(ten);
    expect(values(run(ours, [WATCH], { SYNC_URL: upstream }).out).new).toBe("false");
  });

  // sync-pull takes a branch or a commit too.
  test("is not new when app/ is past it, synced to a commit after the release", () => {
    squash(three);
    expect(values(run(ours, [WATCH], { SYNC_URL: upstream }).out)).toEqual({ tag: "v0.1.10", commit: ten, have: three, new: "false" });
  });

  test("is new when app/ has an earlier release", () => {
    squash(git(upstream, "rev-parse", "v0.1.9"));
    expect(values(run(ours, [WATCH], { SYNC_URL: upstream }).out).new).toBe("true");
  });

  test("a history without the squash commits of app/ is an error, not 'nothing new'", () => {
    const shallow = repo("shallow");
    git(shallow, "commit", "-q", "--allow-empty", "-m", "no squash here");
    const found = run(shallow, [WATCH], { SYNC_URL: upstream });
    expect(found.status).toBe(2);
    expect(found.out).toBe("");
  });
});

describe("the issue", () => {
  const sha = (c: string) => c.repeat(40);
  const base = { TAG: "v0.1.17", COMMIT: sha("b"), HAVE: sha("a"), RESULT: "conflicts", COMMITS: "42", GENERATION: "1", GENERATION_HAVE: "1" };
  let n = 0;
  function issue(env: Record<string, string>) {
    const dir = join(tmp, `issue-${n++}`);
    const done = run(tmp, [WATCH, "issue", dir], env);
    const read = (name: string) => (done.status === 0 ? readFileSync(join(dir, name), "utf8") : "");
    return { status: done.status, title: read("title"), body: read("body.md") };
  }

  test("names the release, links what changed, and lists the files left in conflict", () => {
    const { title, body } = issue({ ...base, FILES: "app/src/window.rs app/src/keys.rs", RUN: "https://example.test/run/1" });
    expect(title).toBe("Sync herdr-app v0.1.17 into app/\n");
    expect(body).toContain("[42 commits since the last sync](https://github.com/herdrdev/herdr-app/compare/aaaaaaaa...v0.1.17)");
    expect(body).toContain("```text\napp/src/window.rs\napp/src/keys.rs\n```");
    expect(body).toContain("app/scripts/yuzora/sync-pull v0.1.17\n");
    expect(body).toContain("[upstream watch](https://example.test/run/1)");
    expect(body).not.toContain("generation");
  });

  test("says so when the release moves the herdr-core generation", () => {
    expect(issue({ ...base, GENERATION: "2" }).body).toContain("**herdr-core generation 1 → 2.**");
  });

  test("a merge with nothing left, and one that did not finish, say that instead of a list", () => {
    expect(issue({ ...base, RESULT: "merged" }).body).toContain("merged with no conflict left for a person");
    expect(issue({ ...base, RESULT: "unfinished", COMMITS: "", GENERATION: "", GENERATION_HAVE: "" }).body).toContain("did not finish");
  });

  // The values come from the job that ran sync-pull on upstream's tree.
  test("prints no file name that is more than a path under app/", () => {
    writeFileSync(join(tmp, "a-file-a-pattern-would-find"), "");
    const { body } = issue({ ...base, FILES: "app/src/a.rs * app/x`y @someone [link](https://example.test) app/src/b.rs" });
    expect(body).toContain("```text\napp/src/a.rs\napp/src/b.rs\n(and 4 more, with names this report does not print)\n```");
    expect(body).not.toMatch(/@someone|example\.test|a-file-a-pattern/);
  });

  test.each([
    ["TAG", "v0.1.17 @someone"],
    ["TAG", "v0.1.17\n## A heading of upstream's"],
    ["COMMIT", "b".repeat(39)],
    ["HAVE", "main"],
    ["RESULT", "current"],
    ["COMMITS", "42](https://example.test)"],
    ["GENERATION", "2**"],
  ])("refuses %s=%j", (name, value) => {
    expect(issue({ ...base, [name]: value }).status).toBe(2);
  });
});

describe("upstream-watch.yml", () => {
  const text = readFileSync(join(resolve(import.meta.dir, "../../.."), ".github/workflows/upstream-watch.yml"), "utf8");
  const workflow = Bun.YAML.parse(text) as any;
  const token = (step: any) => /github\.token|secrets\./.test(JSON.stringify(step));

  test("writes to issues from one job and to nothing else, with no secret", () => {
    expect(workflow.permissions).toEqual({});
    expect(workflow.jobs.look.permissions).toEqual({ contents: "read", issues: "read", "pull-requests": "read" });
    expect(workflow.jobs.issue.permissions).toEqual({ contents: "read", issues: "write" });
    expect(text).not.toContain("secrets.");
  });

  test("the job that runs sync-pull has no token from the merge on", () => {
    const steps = workflow.jobs.look.steps;
    const merge = steps.findIndex((step: any) => step.id === "try");
    expect(merge).toBeGreaterThan(0);
    expect(steps.slice(merge).filter(token)).toEqual([]);
    expect(steps[0].with["persist-credentials"]).toBe(false);
  });

  test("the job that writes takes values, not files, from the other, and opens nothing for a pull request", () => {
    const steps = workflow.jobs.issue.steps;
    expect(steps.map((step: any) => step.uses ?? step.name)).toEqual([steps[0].uses, "Write it", "Open it"]);
    expect(steps[0].uses).toStartWith("actions/checkout@");
    expect(steps[0].with["persist-credentials"]).toBe(false);
    expect(token(steps[1])).toBe(false);
    expect(steps[2].if).toBe("github.event_name != 'pull_request'");
  });

  test("a pull request that changes the watch, or the scripts it runs, runs it", () => {
    expect(workflow.on.pull_request.paths).toEqual([".github/workflows/upstream-watch.yml", "app/scripts/yuzora/sync-watch", "app/scripts/yuzora/sync-pull", "app/scripts/yuzora/sync-resolve.ts"]);
  });
});
