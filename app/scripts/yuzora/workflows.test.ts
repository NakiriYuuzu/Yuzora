import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

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
