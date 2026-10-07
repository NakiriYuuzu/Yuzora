import { execFileSync, spawnSync } from "node:child_process";
import process from "node:process";
import { describe, expect, it } from "vitest";

interface Job {
  name: string;
  if?: string;
  needs?: string[];
  strategy?: { "fail-fast": boolean; matrix: { shard: number[] } };
  steps: { name?: string; run?: string; env?: Record<string, string> }[];
}

const { jobs } = JSON.parse(execFileSync("bun", [
  "-e",
  'console.log(JSON.stringify(Bun.YAML.parse(await Bun.file(".github/workflows/ci.yml").text())))',
], { encoding: "utf8" })) as { jobs: Record<string, Job> };

describe("frontend CI", () => {
  it("keeps the required context and waits for checks and all test shards", () => {
    expect(jobs.frontend.name).toBe("Frontend (lint · typecheck · test · build)");
    expect(jobs.frontend.if).toBe("${{ always() }}");
    expect(jobs.frontend.needs).toEqual(["frontend-checks", "frontend-tests"]);
    expect(jobs.frontend.steps[0].env).toEqual({
      CHECKS_RESULT: "${{ needs.frontend-checks.result }}",
      TESTS_RESULT: "${{ needs.frontend-tests.result }}",
    });
  });

  const results = ["success", "failure", "cancelled", "skipped"];
  it.each(results.flatMap((checks) => results.map((tests) => [checks, tests])))(
    "gates checks=%s and tests=%s",
    (checks, tests) => {
      const result = spawnSync("bash", ["-c", jobs.frontend.steps[0].run ?? "exit 99"], {
        env: { ...process.env, CHECKS_RESULT: checks, TESTS_RESULT: tests },
      });
      expect(result.status).toBe(checks === "success" && tests === "success" ? 0 : 1);
    },
  );

  it("runs every shard with Pages fixtures and retains lint, typecheck and builds", () => {
    const tests = jobs["frontend-tests"];
    const shards = tests.strategy?.matrix.shard ?? [];
    expect(tests.strategy?.["fail-fast"]).toBe(false);
    expect(shards).toEqual([1, 2, 3]);
    expect(tests.steps.at(-1)?.run).toBe(`bun run test --shard=\${{ matrix.shard }}/${shards.length}`);
    expect(tests.needs).toBeUndefined();
    const fixtures = tests.steps.findIndex((step) => step.name === "Build localized Pages and demo");
    expect(fixtures).toBeGreaterThan(-1);
    expect(fixtures).toBeLessThan(tests.steps.length - 1);
    expect(tests.steps[fixtures].run?.trim().split("\n")).toEqual([
      "bun run site:companions", "bun run site:seo", "bun run demo:build",
    ]);
    const checks = jobs["frontend-checks"].steps.map((step) => step.run);
    expect(checks).toContain("bun run lint");
    expect(checks).toContain("bun run typecheck");
    expect(checks).toContain("bun run build");
    expect(checks).toContain(tests.steps[fixtures].run);
  });
});
