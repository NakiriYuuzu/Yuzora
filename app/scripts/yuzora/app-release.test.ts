import { beforeAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { DEFAULT_LOCAL_DOWNLOADS, uploadPlan } from "./release";
import { listedTargets, unlistedTargets } from "./manifest";
import { commandWords } from "./shell-commands";

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
const label = (s: Step) => (s.uses ? s.uses.split("@")[0] : (s.name ?? s.run!));

// The four jobs that write to the repository's releases: when each starts, what it waits for, its steps.
const WRITERS: Record<string, { if: string; needs: string | string[]; steps: string[] }> = {
  draft: {
    if: "needs.plan.outputs.stage == 'draft'",
    needs: "plan",
    steps: ["Helpers", "actions/download-artifact", "Check the files against the run's own manifest", "Tag the commit", "Draft the release and upload"],
  },
  publish: {
    if: "needs.plan.outputs.stage == 'publish'",
    needs: "plan",
    steps: ["Helpers", "Check the draft", "Publish without making it Latest", "Check what the public gets"],
  },
  "upload-manifest": {
    if: "needs.plan.outputs.stage == 'manifest' && needs.verify-manifest.result == 'success'",
    needs: ["plan", "verify-manifest"],
    steps: ["Helpers", "actions/download-artifact", "Upload", "Check what the public gets"],
  },
  latest: {
    if: "needs.plan.outputs.stage == 'latest'",
    needs: "plan",
    steps: ["Helpers", "Set Latest", "Check the links that follow Latest"],
  },
};
// $ok holds true or false; the rest is gh, jq, curl, sha256sum, the helper functions and plain shell.
const WRITER_COMMANDS = ["$ok", "[", "asset_row", "basename", "break", "cat", "cd", "comm", "cp", "curl", "cut", "diff", "echo", "exit", "export", "gh", "grep", "held_sha256", "jq", "local", "ls", "mkdir", "patch_release", "printf", "read", "release_json", "return", "rm", "sed", "set", "sha256_of", "sha256sum", "shift", "sleep", "sort", "tee", "true", "upload_asset"];

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

  test("the jobs that write are four, each starts at its own stage alone, and each has the steps listed and no others", () => {
    expect(writers().map(([name]) => name).sort()).toEqual(Object.keys(WRITERS).sort());
    for (const [name, job] of writers()) {
      expect(job.if, name).toBe(WRITERS[name].if);
      expect(job.needs, name).toEqual(WRITERS[name].needs);
      expect(job.environment, name).toBe("yuzora-release");
      expect(job.permissions?.contents, name).toBe("write");
      expect(job.steps.map(label), name).toEqual(WRITERS[name].steps);
    }
  });

  test("no scope but contents is ever written, and only by those four", () => {
    for (const [name, job] of jobs()) {
      for (const [scope, level] of Object.entries(job.permissions ?? {})) {
        if (level === "write") expect([scope, name in WRITERS], name).toEqual(["contents", true]);
      }
    }
  });

  test("a job that writes runs no repository code and checks nothing out", () => {
    for (const [name, job] of writers()) {
      expect(checksOut(job), name).toBe(false);
      expect(runs(job), name).not.toMatch(/\b(?:bun|bunx|node|npm|cargo|python3?)\b|scripts\/|\.\/node_modules/);
    }
  });

  test("and the only programs it runs are gh, jq, curl, sha256sum and plain shell, whatever files it holds", () => {
    const library = 'source "$RUNNER_TEMP/release-lib.sh"';
    for (const [name, job] of writers()) {
      const ran = new Set(job.steps.flatMap((s) => commandWords((s.run ?? "").replaceAll(library, ""))));
      expect([...ran].filter((command) => !WRITER_COMMANDS.includes(command)), name).toEqual([]);
    }
  });

  test("a job that runs repository code only reads", () => {
    for (const [name, job] of jobs()) {
      if (!checksOut(job)) continue;
      expect(job.permissions?.contents, name).toBe("read");
      for (const step of job.steps.filter((s) => s.uses?.startsWith("actions/checkout@"))) expect(step.with?.["persist-credentials"], name).toBe(false);
    }
  });

  test("the signing secrets are named twice in the whole workflow: two variables of the step that signs", () => {
    // Contexts are case-insensitive, and a key called secrets (a reusable workflow's) counts like a value.
    expect(JSON.stringify(workflow).match(/\bsecrets\b/gi)).toHaveLength(2);
    const sign = workflow.jobs.sign;
    const signing = sign.steps.filter((s) => /\bsecrets\b/i.test(JSON.stringify(s)));
    expect(signing.map((s) => s.name)).toEqual(["Sign"]);
    expect(signing[0].env).toEqual({
      TAURI_SIGNING_PRIVATE_KEY: "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    });
    for (const [name, job] of jobs()) {
      expect(job.uses, name).toBeUndefined();
      expect(job.secrets, name).toBeUndefined();
    }
  });

  test("the job that signs does exactly this, in this order, and runs nothing else with the key", () => {
    const sign = workflow.jobs.sign;
    expect(sign.needs).toEqual(["plan", "build-manifest"]);
    expect(sign.if).toBe("needs.build-manifest.outputs.have_signature != 'true'");
    expect(sign.permissions).toEqual({ contents: "read" });
    expect(sign.environment).toBe("yuzora-release");
    expect(sign.steps.map(label)).toEqual(["actions/checkout", "oven-sh/setup-bun", "bun install --frozen-lockfile --ignore-scripts", "actions/download-artifact", "Sign", "actions/upload-artifact"]);
    const install = sign.steps.find((s) => s.run?.startsWith("bun install"))!;
    expect(install.env).toBeUndefined();
    // The CLI reads the key and password from the environment: nothing secret on a command line.
    const signing = sign.steps.find((s) => s.name === "Sign")!.run!;
    expect(signing).toContain("./node_modules/.bin/tauri signer sign signing/yuzora-update.json");
    expect(signing).not.toMatch(/\s-(?:k|p|f)\b|--private-key|--password/);
    expect([...new Set(commandWords(signing))].sort()).toEqual(["./node_modules/.bin/tauri", "[", "echo", "exit", "set", "test"]);
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
    const calls = verify.split("\n").filter((line) => line.includes('"$verify"'));
    expect(calls).toHaveLength(2);
    for (const call of calls) expect(call, call).toContain('--targets "$targets"');
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

describe("a dry run from the dispatch alone", () => {
  const stepText = (name: string) => workflow.jobs["build-manifest"].steps.find((s) => s.name === name)!.run!;

  // The two build-manifest steps of a dry run, over the files of an App package run (what uploadPlan takes from it, and
  // its unsigned manifest) under the version that run was built as.
  function dryRun(env: { VERSION: string; CARGO_VERSION: string; RUN_ID: string }) {
    const work = mkdtempSync(join(tmpdir(), "yuzora-dryrun-"));
    symlinkSync(join(ROOT, "app"), join(work, "app"));
    mkdirSync(join(work, "dist"));
    if (env.RUN_ID) {
      const files = new Set(uploadPlan(env.CARGO_VERSION).map((p) => p.src));
      for (const name of [...files, "yuzora-update.json"]) writeFileSync(join(work, "dist", name), `${name} of the run`);
    }
    const all = {
      ...process.env,
      STAGE: "dry-run",
      EXCLUDE: unlistedTargets(env.VERSION).join(","),
      DOWNLOADS: DEFAULT_LOCAL_DOWNLOADS,
      PUB_DATE: "2026-11-12T03:04:05+00:00",
      YUZORA_VERSION: env.VERSION,
      ...env,
    };
    const run = (name: string) => spawnSync("bash", ["-c", stepText(name)], { cwd: work, env: all, encoding: "utf8" });
    const named = run("Name them for the version (dry run)");
    const written = named.status === 0 ? run("Write the manifest") : named;
    const manifest = written.status === 0 ? JSON.parse(readFileSync(join(work, "dist/yuzora-update.json"), "utf8")) : null;
    return { status: written.status, err: named.stderr + written.stderr, manifest, files: readdirSync(join(work, "dist")).sort() };
  }

  test("without a package run it signs for made-up archives and lists what the version's table lists", () => {
    const r = dryRun({ VERSION: "0.1.0", CARGO_VERSION: "0.1.0", RUN_ID: "" });
    expect(r.status, r.err).toBe(0);
    expect(Object.keys(r.manifest.targets)).toEqual(["linux-aarch64", "linux-x86_64", "macos-aarch64"]);
    expect(r.manifest.targets["macos-aarch64"].url).toBe(`${DEFAULT_LOCAL_DOWNLOADS}/v0.1.0/yuzora-0.1.0-macos-aarch64.tar.gz`);
    expect(r.files).toContain("yuzora-0.1.0-windows-x86_64.tar.gz");
  });

  test("a candidate built as the version being signed for is used as it is", () => {
    const r = dryRun({ VERSION: "0.1.0", CARGO_VERSION: "0.1.0", RUN_ID: "42" });
    expect(r.status, r.err).toBe(0);
    expect(Object.keys(r.manifest.targets)).toEqual(["linux-aarch64", "linux-x86_64", "macos-aarch64"]);
    expect(r.manifest.notes.url).toBe(`${DEFAULT_LOCAL_DOWNLOADS}/v0.1.0/changelog.md`);
  });

  test("a run built as a beta can be signed for 0.1.0 under the new names, with the old names gone", () => {
    const r = dryRun({ VERSION: "0.1.0", CARGO_VERSION: "0.1.0-beta.4", RUN_ID: "42" });
    expect(r.status, r.err).toBe(0);
    expect(Object.keys(r.manifest.targets)).toEqual(["linux-aarch64", "linux-x86_64", "macos-aarch64"]);
    expect(r.files.filter((f) => f.includes("beta.4"))).toEqual([]);
    expect(r.files).toContain("yuzora-0.1.0-windows-x86_64.tar.gz");
  });

  test("a stable version's dry run lists exactly what the verify step will ask for", () => {
    const r = dryRun({ VERSION: "0.1.0", CARGO_VERSION: "0.1.0", RUN_ID: "" });
    expect(Object.keys(r.manifest.targets).sort()).toEqual([...listedTargets("0.1.0")].sort());
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
# As GitHub does (seen on the v0.1.0 draft): a draft changed without its tag name gets a made-up one.
if [ "\${method:-}" = PATCH ]; then
  id=\${2##*/}; tag=""; draft=""; shift 2
  while [ $# -gt 0 ]; do case "$1" in -f|-F) case "$2" in tag_name=*) tag=\${2#*=};; draft=*) draft=\${2#*=};; esac; shift 2;; *) shift;; esac; done
  if [ -f "$D/tag_is_lost" ]; then tag=""; fi
  jq --argjson i $id --arg t "$tag" --arg d "$draft" 'map(if .id == $i then (if $t != "" then .tag_name = $t elif .draft then .tag_name = "untagged-0123456789abcdef" else . end) | (if $d != "" then .draft = ($d == "true") else . end) else . end)' "$D/releases.json" > "$D/releases.tmp" && mv "$D/releases.tmp" "$D/releases.json"
  exit 0
fi
case "$2" in
  repos/*/releases/*/assets\\?per_page=100) src="$D/assets.json";;
  repos/*/releases\\?per_page=100) if [ -f "$D/fail_list" ]; then echo "gh: HTTP 502 Bad Gateway" >&2; exit 1; fi; src="$D/releases.json";;
  repos/*/releases/assets/*) cat "$D/content_\${2##*/}"; exit 0;;
  repos/*/releases/[0-9]*) jq --argjson i \${2##*/} '.[] | select(.id == $i)' "$D/releases.json" > "$D/one.json"; src="$D/one.json";;
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

  describe("the draft step, when GitHub does not answer the list of releases", () => {
    const draft = (bin: string) =>
      spawnSync("bash", ["-c", workflow.jobs.draft.steps.find((s) => s.name === "Draft the release and upload")!.run!], {
        cwd: dir,
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, RUNNER_TEMP: dir, GH_REPO: "NakiriYuuzu/Yuzora", TAG: "v0.1.0", PRERELEASE: "false", NOTES: "notes", UPLOAD_PLAN: "" },
        encoding: "utf8",
      });
    const calls = () => readFileSync(join(dir, "calls.log"), "utf8");

    test("a failed list is a failure: no release is created on the strength of it", () => {
      const bin = setup([]);
      writeFileSync(join(dir, "fail_list"), "");
      const r = draft(bin);
      expect(r.status).not.toBe(0);
      expect(calls()).not.toContain("release create");
    });

    test("a list without the tag's release is when one is created", () => {
      const bin = setup([]);
      writeFileSync(join(dir, "releases.json"), JSON.stringify([{ id: 9, tag_name: "v0.0.19" }]));
      draft(bin);
      expect(calls()).toContain("release create v0.1.0");
    });
  });

  describe("a change to a release names its tag, or GitHub renames a draft", () => {
    const env = (bin: string) => ({ ...process.env, PATH: `${bin}:${process.env.PATH}`, RUNNER_TEMP: dir, GH_REPO: "NakiriYuuzu/Yuzora", TAG: "v0.1.0", PRERELEASE: "false", NOTES: "notes", UPLOAD_PLAN: "a.txt\ta.txt", RELEASE_ID: "7", GITHUB_STEP_SUMMARY: join(dir, "summary.md") });
    const step = (bin: string, job: string, name: string) => {
      mkdirSync(join(dir, "artifacts"));
      writeFileSync(join(dir, "artifacts/a.txt"), "hello");
      return spawnSync("bash", ["-c", workflow.jobs[job].steps.find((s) => s.name === name)!.run!], { cwd: dir, env: env(bin), encoding: "utf8" });
    };
    const held = () => JSON.parse(readFileSync(join(dir, "releases.json"), "utf8"))[0];
    const calls = () => readFileSync(join(dir, "calls.log"), "utf8");

    test("the workflow changes a release in one place, and that place names the tag", () => {
      const patches = text.split("\n").filter((line) => line.includes("--method PATCH"));
      expect(patches).toHaveLength(1);
      expect(patches[0]).toContain('"repos/$GH_REPO/releases/$id" -f tag_name="$TAG" "$@"');
      expect(text).not.toMatch(/gh release edit/);
    });

    test("the draft stage run again leaves the draft under its tag", () => {
      const bin = setup([]);
      const r = step(bin, "draft", "Draft the release and upload");
      expect(r.status, r.stderr).toBe(0);
      expect(calls()).toContain("--method PATCH repos/NakiriYuuzu/Yuzora/releases/7 -f tag_name=v0.1.0 -F body=@");
      expect(calls()).not.toContain("release create");
      expect(calls()).toContain("release upload v0.1.0");
      expect(held()).toMatchObject({ tag_name: "v0.1.0", draft: true });
    });

    test("publishing makes the draft public under its tag", () => {
      const bin = setup([]);
      const r = step(bin, "publish", "Publish without making it Latest");
      expect(r.status, r.stderr).toBe(0);
      expect(calls()).toContain("--method PATCH repos/NakiriYuuzu/Yuzora/releases/7 -f tag_name=v0.1.0 -F draft=false -F prerelease=false -f make_latest=false");
      expect(held()).toMatchObject({ tag_name: "v0.1.0", draft: false });
    });

    test("a release that ends up under another tag stops the stage, for each of the three changes", () => {
      for (const [job, name] of [["draft", "Draft the release and upload"], ["publish", "Publish without making it Latest"]]) {
        const bin = setup([]);
        writeFileSync(join(dir, "tag_is_lost"), "");
        const r = step(bin, job, name);
        expect(r.status, job).not.toBe(0);
        expect(r.stderr, job).toContain("is under the tag untagged-0123456789abcdef after the change, not v0.1.0");
        expect(calls(), job).not.toContain("release upload");
      }
      const bin = setup([]);
      const r = spawnSync("bash", ["-c", `source ${lib}; patch_release 7 -f make_latest=true`], { env: env(bin), encoding: "utf8" });
      expect(r.status, r.stderr).toBe(0);
      expect(runs(workflow.jobs.latest)).toContain('patch_release "$(jq -r .id <<<"$release")" -f make_latest=true');
    });
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
