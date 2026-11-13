import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { uploadPlan } from "./release";

// Where the app's own workflows run: main (once integration lands there) and
// beta2/integration, and who saves the build caches.
const WORKFLOWS = join(resolve(import.meta.dir, "../../.."), ".github/workflows");
const load = (name: string) => Bun.YAML.parse(readFileSync(join(WORKFLOWS, name), "utf8")) as any;
const BRANCHES = ["main", "beta2/integration"];

describe.each(["app-ci.yml", "app-package.yml"])("%s", (file) => {
  test("runs for pushes and pull requests on main and on integration", () => {
    const workflow = load(file);
    expect(workflow.on.push.branches).toEqual(BRANCHES);
    expect(workflow.on.pull_request.branches).toEqual(BRANCHES);
  });

  test("keeps its path filter on both events, so a docs-only push does not rebuild four platforms", () => {
    const workflow = load(file);
    expect(workflow.on.push.paths.length).toBeGreaterThan(3);
    expect(workflow.on.pull_request.paths).toEqual(workflow.on.push.paths);
  });
});

describe("caches", () => {
  test("only a push to main or to integration saves them; a pull request only reads", () => {
    const text = readFileSync(join(WORKFLOWS, "app-ci.yml"), "utf8");
    const env = load("app-ci.yml").env;
    expect(env.SAVE_CACHES).toBe("${{ github.event_name == 'push' && (github.ref == 'refs/heads/main' || github.ref == 'refs/heads/beta2/integration') }}");
    // Every save is behind that switch and an exact-key miss.
    const saves = text.match(/uses: actions\/cache\/save@/g) ?? [];
    expect(saves).toHaveLength(2);
    expect(text.match(/if: env\.SAVE_CACHES == 'true' && steps\.cache\.outputs\.cache-hit != 'true'/g)).toHaveLength(4);
  });

  test("one job drops the caches a save replaced: it alone may write to Actions, only on a push, and it runs nothing from the repository", () => {
    const ci = load("app-ci.yml");
    const writers = Object.keys(ci.jobs).filter((name) => ci.jobs[name].permissions?.actions === "write");
    expect(writers).toEqual(["prune-caches"]);
    const prune = ci.jobs["prune-caches"];
    expect(prune.if).toContain("github.event_name == 'push'");
    expect(prune.steps.filter((step: any) => step.uses)).toEqual([]);
    // Every cache a job saves: one per platform of the matrix, and the Windows job's.
    const prefix = (job: string) => ci.jobs[job].steps.find((step: any) => step.id === "cache").with.key.split("${{")[0];
    const saved = ci.jobs["platform-tests"].strategy.matrix.include.map((m: any) => `${prefix("platform-tests")}${m.platform}-`);
    for (const name of [...saved, prefix("windows-console")]) expect(prune.steps[0].run).toContain(name);
    expect(ci.jobs.app.needs).not.toContain("prune-caches");
  });

  test("a run that saves starts from no older cache, so what it saves is one build and not every build before it", () => {
    const ci = load("app-ci.yml");
    expect(ci.env.RESTORE_OLDER).toBe("${{ github.event_name != 'push' }}");
    for (const job of ["platform-tests", "windows-console"]) {
      const restore = ci.jobs[job].steps.find((step: any) => step.id === "cache");
      expect(restore.with["restore-keys"], job).toStartWith("${{ env.RESTORE_OLDER == 'true' && ");
    }
  });

  test("the jobs on the workspace's cache build what it holds: no -p (Cargo would resolve other features) and no other profile", () => {
    const ci = load("app-ci.yml");
    for (const job of ["platform-tests", "database-integration"]) {
      const runs = ci.jobs[job].steps.map((step: any) => step.run ?? "").filter((run: string) => run.includes("cargo test") && !run.includes("--manifest-path"));
      expect(runs.length, job).toBeGreaterThan(1);
      for (const run of runs) {
        expect(run, job).not.toMatch(/\s-p\s|--package/);
        expect(run, job).not.toContain("opt-level");
      }
    }
  });

  test("app-package.yml uses none: its release targets would push out the others", () => {
    expect(readFileSync(join(WORKFLOWS, "app-package.yml"), "utf8")).not.toMatch(/uses: actions\/cache/);
  });
});

describe("the manifest checker the release uses", () => {
  test("its own tests (and manifest.rs's, which it compiles) run in App CI on Linux", () => {
    const ci = load("app-ci.yml");
    const step = ci.jobs["platform-tests"].steps.find((s: any) => s.name === "Check the manifest checker");
    expect(step.if).toBe("matrix.platform == 'linux-x86_64'");
    expect(step.run).toContain("cargo test --locked --manifest-path scripts/yuzora/verify-manifest/Cargo.toml");
  });
});

describe("the release workflow's contract is run by the PR that changes the workflow", () => {
  const ci = load("app-ci.yml");

  test("a change to app-release.yml starts App CI, as does anything under app/ the contract reads", () => {
    for (const event of ["push", "pull_request"]) {
      expect(ci.on[event].paths, event).toContain(".github/workflows/app-release.yml");
      expect(ci.on[event].paths, event).toContain("app/**");
    }
  });

  test("and App CI's Linux job runs every test under scripts, the contract's among them", () => {
    const step = ci.jobs["platform-tests"].steps.find((s: any) => s.name === "Yuzora script tests");
    expect(step.if).toBe("matrix.platform == 'linux-x86_64'");
    expect(step.run).toBe("bun test scripts");
    expect(existsSync(join(import.meta.dir, "app-release.test.ts"))).toBe(true);
  });
});

describe("the website's download links", () => {
  const downloads = readFileSync(join(WORKFLOWS, "../../site/downloads.js"), "utf8");

  test("name files a stable release carries, each by its fixed name under the latest release", () => {
    const carried = uploadPlan("0.1.1").map((p) => p.dest);
    expect(downloads).toContain("releases/latest/download/");
    expect(downloads).not.toMatch(/releases\/download\/v/);
    const named = [...downloads.matchAll(/\$\{RELEASE_DOWNLOAD_BASE\}(Yuzora-[\w.-]+)`/g)].map((m) => m[1]);
    expect(named).toEqual(["Yuzora-windows-x64-setup.exe", "Yuzora-macos-aarch64.dmg"]);
    for (const name of named) expect(carried).toContain(name);
    // No MSI: 0.1.0 ships an NSIS installer only.
    expect(downloads).not.toContain(".msi");
  });
});
