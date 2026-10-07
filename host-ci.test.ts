import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";

type Workflow = {
  on: Record<string, { inputs?: Record<string, unknown> } | null>;
  jobs: Record<string, { if?: string; uses?: string; with?: Record<string, string> }>;
};

const load = (file: string) => JSON.parse(execFileSync("bun", [
  "-e",
  `console.log(JSON.stringify(Bun.YAML.parse(await Bun.file(${JSON.stringify(file)}).text())))`,
], { encoding: "utf8" })) as Workflow;

describe("host helper CI", () => {
  // The direct pull_request trigger skips release PRs only because CI builds
  // their helpers through the reusable call. Dropping `caller` from that call
  // would skip the helpers there too and silently remove the release candidates.
  it("skips duplicate release PR runs only when CI names itself as the caller", () => {
    const host = load(".github/workflows/host.yml");
    const ci = load(".github/workflows/ci.yml");
    expect(host.on.workflow_call?.inputs).toHaveProperty("caller");
    expect(host.jobs.build.if).toBe(
      "${{ inputs.caller != '' || github.event_name != 'pull_request' || !startsWith(github.head_ref, 'release/') }}",
    );
    expect(ci.jobs["host-artifacts"].uses).toBe("./.github/workflows/host.yml");
    expect(ci.jobs["host-artifacts"].with?.caller).toBe("ci");
  });
});
