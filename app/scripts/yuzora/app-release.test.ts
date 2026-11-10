import { beforeAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

// The release workflow's contract (.github/workflows/app-release.yml): which
// job can write, which can read the signing key, what runs repository code.
const ROOT = resolve(import.meta.dir, "../../..");
const WORKFLOWS = join(ROOT, ".github/workflows");

type Step = { name?: string; uses?: string; run?: string; env?: Record<string, string>; with?: Record<string, unknown>; if?: string };
type Job = { name: string; needs?: string | string[]; if?: string; environment?: string; permissions?: Record<string, string>; steps: Step[]; env?: Record<string, string> };
let workflow: { on: Record<string, any>; permissions: Record<string, string>; jobs: Record<string, Job> };
let text: string;

beforeAll(() => {
  text = readFileSync(join(WORKFLOWS, "app-release.yml"), "utf8");
  workflow = Bun.YAML.parse(text) as typeof workflow;
});

const jobs = () => Object.entries(workflow.jobs);
const runs = (job: Job) => job.steps.map((s) => s.run ?? "").join("\n");
const writers = () => jobs().filter(([, job]) => job.permissions?.contents === "write");
const checksOut = (job: Job) => job.steps.some((s) => s.uses?.startsWith("actions/checkout@"));

describe("who can do what", () => {
  test("it starts by hand and nothing else: no push, so no branch can start a run by being pushed", () => {
    expect(Object.keys(workflow.on)).toEqual(["workflow_dispatch"]);
    expect(workflow.permissions).toEqual({ contents: "read" });
    expect(workflow.on.workflow_dispatch.inputs.stage.options).toEqual(["dry-run", "draft", "publish", "manifest", "latest"]);
    expect(text).not.toContain("event_name");
  });

  test("a dry run takes its stage and version from the dispatch, like every other stage", () => {
    const planStep = workflow.jobs.plan.steps.find((s) => s.name === "Check the inputs, the run and the tag")!;
    expect(planStep.env!.STAGE).toBe("${{ inputs.stage }}");
    expect(planStep.env!.VERSION).toBe("${{ inputs.version }}");
  });

  test("which targets the manifest lists is not a dispatch input: the repository's table decides, for the build and for the check", () => {
    expect(Object.keys(workflow.on.workflow_dispatch.inputs).sort()).toEqual(["downloads_base", "package_run_id", "stage", "version"]);
    const planStep = workflow.jobs.plan.steps.find((s) => s.name === "Check the inputs, the run and the tag")!;
    expect(Object.keys(planStep.env!).sort()).toEqual(["DOWNLOADS_BASE", "GH_TOKEN", "PACKAGE_RUN_ID", "STAGE", "VERSION"]);
    const verify = workflow.jobs["verify-manifest"];
    expect(verify.env).toEqual({ STAGE: "${{ needs.plan.outputs.stage }}", VERSION: "${{ needs.plan.outputs.version }}", DOWNLOADS: "${{ needs.plan.outputs.downloads }}" });
    expect(runs(verify)).toContain('import { listedTargets } from "./app/scripts/yuzora/manifest.ts"; console.log(listedTargets(process.env.VERSION ?? "").join(","))');
  });

  test("no job that writes can start in a dry run", () => {
    const stages: Record<string, string> = { draft: "draft", publish: "publish", "upload-manifest": "manifest", latest: "latest" };
    expect(writers().map(([name]) => name).sort()).toEqual(Object.keys(stages).sort());
    for (const [name, job] of writers()) {
      expect(job.if, name).toContain(`needs.plan.outputs.stage == '${stages[name]}'`);
      expect(job.if, name).not.toContain("dry-run");
      expect(job.environment, name).toBe("yuzora-release");
    }
  });

  test("a job that writes runs no repository code and checks nothing out", () => {
    for (const [name, job] of writers()) {
      expect(checksOut(job), name).toBe(false);
      expect(runs(job), name).not.toMatch(/\b(?:bun|bunx|node|npm|cargo|python3?)\b|scripts\/|\.\/node_modules/);
      expect(JSON.stringify(job), name).not.toContain("secrets.");
    }
  });

  test("a job that runs repository code only reads", () => {
    for (const [name, job] of jobs()) {
      if (!checksOut(job)) continue;
      expect(job.permissions?.contents, name).toBe("read");
      for (const step of job.steps.filter((s) => s.uses?.startsWith("actions/checkout@"))) expect(step.with?.["persist-credentials"], name).toBe(false);
    }
  });

  test("only the sign job names the signing secrets, and only in the step that signs", () => {
    const holders = jobs().filter(([, job]) => JSON.stringify(job).includes("secrets.")).map(([name]) => name);
    expect(holders).toEqual(["sign"]);
    expect(text.match(/secrets\./g)).toHaveLength(2);
    expect(text).toContain("secrets.TAURI_SIGNING_PRIVATE_KEY }}");
    expect(text).toContain("secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}");
    const sign = workflow.jobs.sign;
    expect(sign.permissions).toEqual({ contents: "read" });
    expect(sign.environment).toBe("yuzora-release");
    const withSecrets = sign.steps.filter((s) => JSON.stringify(s).includes("secrets."));
    expect(withSecrets.map((s) => s.name)).toEqual(["Sign"]);
    const install = sign.steps.find((s) => s.run?.startsWith("bun install"))!;
    expect(install.run).toContain("--frozen-lockfile --ignore-scripts");
    expect(install.env).toBeUndefined();
    // The CLI reads the key and password from the environment: nothing secret on a command line.
    const signing = withSecrets[0].run!;
    expect(signing).toContain("./node_modules/.bin/tauri signer sign signing/yuzora-update.json");
    expect(signing).not.toMatch(/\s-(?:k|p|f)\b|--private-key|--password/);
  });

  test("every action is pinned to a commit, and no script takes a workflow expression", () => {
    for (const [name, job] of jobs()) {
      for (const step of job.steps) {
        if (step.uses) expect(step.uses, name).toMatch(/^[^@\s]+@[a-f0-9]{40}$/);
        if (step.run) expect(step.run, `${name}: ${step.name}`).not.toContain("${{");
      }
    }
    expect(text).toMatch(/actions\/checkout@[a-f0-9]{40} # v/);
  });

  test("the checks of the manifest use the app's own code, and nothing trusts a test key", () => {
    const verify = runs(workflow.jobs["verify-manifest"]);
    expect(verify).toContain("cd app/scripts/yuzora/verify-manifest");
    // The repository's cargo config sends builds to src-tauri/target: the binary is run from its own.
    expect(verify).toContain('cargo build --locked --release --target-dir "$PWD/target"');
    expect(verify).toContain("--targets");
    for (const file of readdirSync(WORKFLOWS)) expect(readFileSync(join(WORKFLOWS, file), "utf8"), file).not.toContain("--extra-key");
  });

  test("a dry run's manifest is the only one that names a server of its own", () => {
    const write = workflow.jobs["build-manifest"].steps.find((s) => s.name === "Write the manifest")!.run!;
    expect(write).toContain('if [ "$STAGE" = dry-run ]; then export YUZORA_DOWNLOADS="$DOWNLOADS/v$VERSION"; else unset YUZORA_DOWNLOADS; fi');
    expect(workflow.jobs["upload-manifest"].needs).toEqual(["plan", "verify-manifest"]);
  });

  test("every script is valid bash", () => {
    for (const [name, job] of jobs()) {
      for (const step of job.steps.filter((s) => s.run)) {
        const r = spawnSync("bash", ["-n"], { input: step.run, encoding: "utf8" });
        expect(r.status, `${name}: ${step.name}\n${r.stderr}`).toBe(0);
      }
    }
  });
});

describe("the order of things that cannot be taken back", () => {
  test("the draft is a draft, never Latest, and its tag is made through the API", () => {
    const draft = workflow.jobs.draft;
    const script = runs(draft);
    expect(script).toContain("--draft --verify-tag");
    expect(script).toContain("--latest=false");
    expect(script).toContain('gh api --method POST "repos/$GH_REPO/git/tags"');
    expect(script).toContain('gh api --method POST "repos/$GH_REPO/git/refs"');
    expect(script).not.toMatch(/git push|git tag|--clobber/);
    expect(script).not.toContain("yuzora-update.json.sig");
  });

  test("publishing says make_latest false, because the default would make it Latest", () => {
    const publish = runs(workflow.jobs.publish);
    expect(publish).toContain("-F draft=false");
    expect(publish).toContain("-f make_latest=false");
    expect(publish).toContain("the draft already has the manifest or its signature");
    expect(publish).not.toContain("make_latest=true");
  });

  test("the signature goes up after the manifest, and Latest is its own stage", () => {
    const upload = workflow.jobs["upload-manifest"].steps.find((s) => s.name === "Upload")!.run!;
    expect(upload.indexOf("upload_asset signed/yuzora-update.json\n")).toBeGreaterThan(-1);
    expect(upload.indexOf("upload_asset signed/yuzora-update.json.sig")).toBeGreaterThan(upload.indexOf("upload_asset signed/yuzora-update.json\n"));
    const latest = runs(workflow.jobs.latest);
    expect(latest).toContain("-f make_latest=true");
    for (const other of ["draft", "publish", "upload-manifest"]) expect(runs(workflow.jobs[other])).not.toContain("make_latest=true");
  });

  test("the four jobs that write share one helper, written once", () => {
    const helpers = ["draft", "publish", "upload-manifest", "latest"].map((name) => workflow.jobs[name].steps[0]);
    for (const step of helpers) expect(step.run).toBe(helpers[0].run);
    expect(helpers[0].name).toBe("Helpers");
  });
});

describe("the helper that uploads a file once", () => {
  let dir: string;
  let lib: string;

  // A fake gh over a folder: releases.json, assets.json (release 7's), content_<id>, calls.log.
  function setup(assets: unknown[], contents: Record<string, string> = {}) {
    dir = mkdtempSync(join(tmpdir(), "yuzora-lib-"));
    const bin = join(dir, "bin");
    mkdirSync(bin);
    writeFileSync(join(dir, "releases.json"), JSON.stringify([{ id: 7, tag_name: "v0.1.0", draft: true, assets }]));
    writeFileSync(join(dir, "assets.json"), JSON.stringify(assets));
    for (const [id, body] of Object.entries(contents)) writeFileSync(join(dir, `content_${id}`), body);
    writeFileSync(join(bin, "sha256sum"), '#!/bin/sh\nexec shasum -a 256 "$@"\n');
    writeFileSync(
      join(bin, "gh"),
      `#!/bin/bash
D="${dir}"
echo "gh $*" >> "$D/calls.log"
jq_expr=""; args=(); while [ $# -gt 0 ]; do case "$1" in --jq) jq_expr=$2; shift 2;; --paginate) shift;; -H) shift 2;; --method) method=$2; shift 2;; *) args+=("$1"); shift;; esac; done
set -- "\${args[@]}"
if [ "$1" = release ] && [ "$2" = upload ]; then
  f=$4; name=$(basename "$f"); id=$((100 + $(jq length "$D/assets.json")))
  cp "$f" "$D/content_$id"
  digest=$(shasum -a 256 "$f" | cut -d' ' -f1)
  jq --arg n "$name" --argjson i $id --arg d "sha256:$digest" '. + [{id:$i, name:$n, state:"uploaded", digest:$d}]' "$D/assets.json" > "$D/assets.tmp" && mv "$D/assets.tmp" "$D/assets.json"
  exit 0
fi
if [ "\${method:-}" = DELETE ]; then id=\${2##*/}; jq --argjson i $id 'map(select(.id != $i))' "$D/assets.json" > "$D/assets.tmp" && mv "$D/assets.tmp" "$D/assets.json"; exit 0; fi
case "$2" in
  repos/*/releases/*/assets\\?per_page=100) src="$D/assets.json";;
  repos/*/releases\\?per_page=100) src="$D/releases.json";;
  repos/*/releases/assets/*) cat "$D/content_\${2##*/}"; exit 0;;
  *) echo "fake gh: unexpected $*" >&2; exit 9;;
esac
if [ -n "$jq_expr" ]; then jq -r "$jq_expr" "$src"; else cat "$src"; fi
`,
    );
    chmodSync(join(bin, "gh"), 0o755);
    chmodSync(join(bin, "sha256sum"), 0o755);
    const helpers = workflow.jobs.draft.steps[0].run!;
    const r = spawnSync("bash", ["-c", helpers], { env: { ...process.env, RUNNER_TEMP: dir }, encoding: "utf8" });
    expect(r.status, r.stderr).toBe(0);
    lib = join(dir, "release-lib.sh");
    return bin;
  }

  function upload(bin: string, name: string, body: string) {
    const file = join(dir, name);
    writeFileSync(file, body);
    const r = spawnSync("bash", ["-c", `source ${lib}; RELEASE_ID=7; upload_asset ${file}`], {
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GH_REPO: "NakiriYuuzu/Yuzora", TAG: "v0.1.0" },
      encoding: "utf8",
    });
    return { status: r.status, out: r.stdout, err: r.stderr, calls: readFileSync(join(dir, "calls.log"), "utf8") };
  }

  const sha = (body: string) => new Bun.CryptoHasher("sha256").update(body).digest("hex");

  test("a file that is not there is uploaded and then found with its sha256", () => {
    const bin = setup([]);
    const r = upload(bin, "a.txt", "hello");
    expect(r.status, r.err).toBe(0);
    expect(r.out).toContain(`uploaded  a.txt ${sha("hello")}`);
    expect(r.calls).toContain("release upload v0.1.0");
  });

  test("a file already there with the same content is left alone", () => {
    const bin = setup([{ id: 5, name: "a.txt", state: "uploaded", digest: `sha256:${sha("hello")}` }]);
    const r = upload(bin, "a.txt", "hello");
    expect(r.status, r.err).toBe(0);
    expect(r.out).toContain("same      a.txt");
    expect(r.calls).not.toContain("release upload");
  });

  test("other content under the same name stops the stage, and nothing is replaced", () => {
    const bin = setup([{ id: 5, name: "a.txt", state: "uploaded", digest: `sha256:${sha("old")}` }]);
    const r = upload(bin, "a.txt", "new");
    expect(r.status).not.toBe(0);
    expect(r.out + r.err).toContain("already on v0.1.0 with other content");
    expect(r.calls).not.toMatch(/release upload|DELETE/);
  });

  test("without a digest GitHub's copy is downloaded and compared", () => {
    const bin = setup([{ id: 5, name: "a.txt", state: "uploaded" }], { "5": "hello" });
    expect(upload(bin, "a.txt", "hello").out).toContain("same      a.txt");
    const bad = upload(bin, "a.txt", "other");
    expect(bad.status).not.toBe(0);
  });

  test("an upload that never finished is removed and done again", () => {
    const bin = setup([{ id: 5, name: "a.txt", state: "starter" }]);
    const r = upload(bin, "a.txt", "hello");
    expect(r.status, r.err).toBe(0);
    expect(r.calls).toContain("--method DELETE");
    expect(r.out).toContain("uploaded  a.txt");
  });

  test("the release for a tag, drafts included, must be one", () => {
    const bin = setup([]);
    const run = () => spawnSync("bash", ["-c", `source ${lib}; release_json | jq -r .id`], { env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GH_REPO: "NakiriYuuzu/Yuzora", TAG: "v0.1.0" }, encoding: "utf8" });
    expect(run().stdout.trim()).toBe("7");
    writeFileSync(join(dir, "releases.json"), JSON.stringify([{ id: 7, tag_name: "v0.1.0" }, { id: 8, tag_name: "v0.1.0" }]));
    const two = run();
    expect(two.status).not.toBe(0);
    expect(two.stderr).toContain("more than one release");
    writeFileSync(join(dir, "releases.json"), JSON.stringify([{ id: 9, tag_name: "v0.0.19" }]));
    expect(run().stdout.trim()).toBe("");
  });
});
