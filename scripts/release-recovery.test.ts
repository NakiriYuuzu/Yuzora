import { execFileSync, spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { afterEach, describe, expect, it } from "vitest"

interface Step { name?: string; uses?: string; run?: string; env?: Record<string, string>; with?: Record<string, unknown> }
interface Job { permissions: Record<string, string>; steps: Step[]; if?: string }
const workflow = JSON.parse(execFileSync("bun", ["-e", 'import {parseReleaseWorkflow} from "./scripts/release-contract"; console.log(JSON.stringify(parseReleaseWorkflow(await Bun.file(".github/workflows/recover-stable-release.yml").text())))'], { encoding: "utf8" })) as { jobs: Record<string, Job> }
const temporary: string[] = []
afterEach(() => { for (const directory of temporary.splice(0)) rmSync(directory, { recursive: true, force: true }) })
const source = "a".repeat(40)
const fixtureCli = `#!/usr/bin/env bun
import {writeFileSync} from "node:fs";
const args=process.argv.slice(2),path=args[1]||"",mode=process.env.FIXTURE_MODE;
const sha="a".repeat(40),digest="sha256:"+"c".repeat(64);
const names=["Main CI / release channel","host-artifacts / build (ubuntu-24.04, linux-x86_64)","host-artifacts / build (ubuntu-24.04-arm, linux-aarch64)","host-artifacts / build (macos-14, macos-aarch64)","host-artifacts / build (macos-15-intel, macos-x86_64)","Build macOS Apple Silicon installers","Build Windows x86-64 installers","Assemble draft release from build artifacts"];
const assets=["Yuzora_0.0.9_aarch64.dmg","Yuzora_0.0.9_aarch64.app.tar.gz","Yuzora_0.0.9_aarch64.app.tar.gz.sig","Yuzora_0.0.9_x64-setup.exe","Yuzora_0.0.9_x64-setup.exe.sig","Yuzora_0.0.9_x64_en-US.msi","Yuzora_0.0.9_x64_en-US.msi.sig","Yuzora-macos-aarch64.dmg","Yuzora-windows-x64-setup.exe","Yuzora-windows-x64.msi"].map((name,id)=>({id,name,size:100,digest}));
if(mode==="changed_asset")assets[0].digest="sha256:"+"d".repeat(64);
const draft={id:456,tag_name:"v0.0.9",draft:mode!=="published",prerelease:false,body:mode==="changed_notes"?"Different notes":"Stable notes",assets:mode==="missing_asset"?assets.slice(1):assets};
const emit=x=>console.log(JSON.stringify(x));
if(args[0]==="release"){
 if(args[1]!=="download")throw new Error("Unexpected mutation: "+args.join(" "));
 for(const asset of assets.filter(x=>x.name.endsWith(".sig")))writeFileSync(args[args.indexOf("--dir")+1]+"/"+asset.name,"signature");
}else if(args.includes("--slurp")&&args.some(x=>x.includes("/jobs?"))){
 emit([{jobs:names.map(name=>({name,conclusion:mode==="failed_build"&&name==="Build Windows x86-64 installers"?"failure":"success"}))}]);
}else if(args.includes("--slurp")&&args.some(x=>x.includes("/releases?"))){emit([[draft]]);
}else if(path.includes("/actions/workflows/ci.yml/runs?")){
 emit({workflow_runs:mode==="missing_ci"?[]:[{head_sha:sha,event:"push",head_branch:"main",conclusion:"success"}]});
}else if(path.includes("/actions/runs/")){
 emit({path:".github/workflows/release.yml",event:mode==="wrong_event"?"pull_request":"workflow_run",head_branch:"main",head_sha:sha,conclusion:"failure"});
}else if(path.includes("/contents/package.json")){
 console.log(Buffer.from(JSON.stringify({version:mode==="beta"?"0.0.9-beta.1":"0.0.9"})).toString("base64"));
}else if(path.includes("/git/ref/tags/")){emit({object:{type:"tag",sha:"b".repeat(40)}});
}else if(path.includes("/git/tags/")){emit({object:{type:"commit",sha:mode==="wrong_tag"?"d".repeat(40):sha}});
}else if(path.includes("/releases/456")){emit(draft);
}else {throw new Error("Unexpected gh command: "+args.join(" "))}
`

function runGuard(mode: string) {
  const directory = mkdtempSync(join(tmpdir(), "yuzora-release-recovery-"))
  temporary.push(directory)
  writeFileSync(join(directory, "gh"), fixtureCli, { mode: 0o755 })
  const script = workflow.jobs.guard.steps.find((step) => step.name === "Validate original build and capture draft inputs")?.run
  if (!script) throw new Error("Missing recovery guard")
  const output = join(directory, "outputs")
  const result = spawnSync("bash", ["-c", script], {
    cwd: directory,
    encoding: "utf8",
    env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, FIXTURE_MODE: mode, GH_REPO: "NakiriYuuzu/Yuzora", SOURCE_RUN_ID: "123", WORKFLOW_SHA: source, GITHUB_OUTPUT: output },
  })
  return { ...result, output, directory }
}

describe("stable release recovery", () => {
  it("executes the actual shell guard against a complete source-bound draft", () => {
    const result = runGuard("valid")
    expect(result.status, result.stderr).toBe(0)
    expect(readFileSync(result.output, "utf8")).toContain(`source_sha=${source}`)
    expect(readFileSync(result.output, "utf8")).toContain("tag_name=v0.0.9")
  })

  it.each(["wrong_event", "missing_ci", "failed_build", "wrong_tag", "published", "missing_asset", "beta"])("stops recovery for %s", (mode) => {
    expect(runGuard(mode).status).not.toBe(0)
  })

  it.each(["valid", "changed_asset", "changed_notes", "published", "wrong_tag"])("rechecks the draft immediately before writing: %s", (mode) => {
    const fixture = runGuard("valid")
    expect(fixture.status, fixture.stderr).toBe(0)
    const script = workflow.jobs["publish-release"].steps.find((step) => step.name === "Recheck immutable source and unchanged draft before uploading metadata")?.run
    if (!script) throw new Error("Missing publication recheck")
    const result = spawnSync("bash", ["-c", script.slice(0, script.indexOf("gh release upload"))], {
      cwd: fixture.directory,
      encoding: "utf8",
      env: { ...process.env, PATH: `${fixture.directory}:${process.env.PATH}`, FIXTURE_MODE: mode, GH_REPO: "NakiriYuuzu/Yuzora", SOURCE_SHA: source, TAG_NAME: "v0.0.9", RELEASE_ID: "456" },
    })
    if (mode === "valid") expect(result.status, result.stderr).toBe(0)
    else expect(result.status).not.toBe(0)
  })

  it("keeps repository execution read-only and publication restricted to reviewed main", () => {
    expect(workflow.jobs.guard.if).toBe("github.ref == 'refs/heads/main'")
    for (const job of Object.values(workflow.jobs)) {
      if (job.permissions.contents === "write") {
        expect(job.steps.some((step) => step.uses?.startsWith("actions/checkout@"))).toBe(false)
        expect(job.steps.some((step) => /bun scripts\//.test(step.run ?? ""))).toBe(false)
      }
      for (const step of job.steps) if (step.uses) expect(step.uses).toMatch(/@[0-9a-f]{40}$/)
    }
    const prepare = workflow.jobs["prepare-updater-metadata"]
    expect(prepare.permissions.contents).toBe("read")
    expect(prepare.steps.some((step) => step.env?.GH_TOKEN)).toBe(false)
    expect(prepare.steps.find((step) => step.uses?.startsWith("actions/checkout@"))?.with?.ref).toBe("${{ needs.guard.outputs.source_sha }}")
    const publish = workflow.jobs["publish-release"].steps
    const check = publish.find((step) => step.name === "Recheck immutable source and unchanged draft before uploading metadata")?.run ?? ""
    expect(check.indexOf("cmp release-inputs/release.json.canonical")).toBeLessThan(check.indexOf("gh release upload"))
    expect(publish.find((step) => step.name === "Verify release assets and updater metadata")?.run).toContain("exact allowlist mismatch")
  })
})
