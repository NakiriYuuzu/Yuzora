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
