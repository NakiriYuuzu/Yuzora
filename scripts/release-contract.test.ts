import { execFileSync, spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"

import { describe, expect, it } from "vitest"

import {
  apiLookupIsAuthoritativeNotFound,
  betaReleaseAssetNamesAreSafe,
  isPinnedReleaseActionRef,
} from "./release-contract"

interface WorkflowStep {
  env?: Record<string, string>
  name?: string
  run?: string
  uses?: string
}

interface WorkflowJob {
  env?: Record<string, string>
  steps: WorkflowStep[]
}

interface ParsedReleaseWorkflow {
  jobs: Record<string, WorkflowJob>
}

function verify(script: string) {
  return execFileSync("bun", [script], { encoding: "utf8" })
}

function releaseWorkflow(): ParsedReleaseWorkflow {
  return JSON.parse(
    execFileSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow } from "./scripts/release-contract.ts";
        console.log(JSON.stringify(parseReleaseWorkflow(await Bun.file(".github/workflows/release.yml").text())));
      `],
      { encoding: "utf8" }
    )
  ) as ParsedReleaseWorkflow
}

describe("release workflow contracts", () => {
  it("keeps stable updater release guarantees", () => {
    expect(verify("scripts/verify-updater-release-contract.ts")).toContain(
      "Stable updater release contract verified"
    )
  })

  it("keeps beta release isolated from stable updater assets and secrets", () => {
    expect(verify("scripts/verify-beta-release-contract.ts")).toContain(
      "Beta prerelease contract verified"
    )
  })

  it("rejects mutable action refs and accepts full commit pins", () => {
    expect(isPinnedReleaseActionRef("actions/checkout@v4")).toBe(false)
    expect(isPinnedReleaseActionRef("dtolnay/rust-toolchain@stable")).toBe(false)
    expect(
      isPinnedReleaseActionRef(
        "actions/checkout@11d5960a326750d5838078e36cf38b85af677262"
      )
    ).toBe(true)
  })

  it("locks every checkout step to the same reviewed upstream commit", () => {
    const result = spawnSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow, verifyStableReleaseContract } from "./scripts/release-contract.ts";
        const workflow = parseReleaseWorkflow(await Bun.file(".github/workflows/release.yml").text());
        workflow.jobs.guard.steps.find((step) => step.uses?.startsWith("actions/checkout@"))
          .uses = "actions/checkout@0000000000000000000000000000000000000000";
        verifyStableReleaseContract(workflow);
      `],
      { encoding: "utf8" }
    )

    expect(result.status).not.toBe(0)
    expect(result.stderr).toContain("reviewed actions/checkout commit")
  })

  it("gives every no-checkout gh release job an explicit repository", () => {
    const workflow = releaseWorkflow()
    const ghReleaseJobs = Object.entries(workflow.jobs)
      .filter(([, job]) =>
        !job.steps.some((step) => step.uses?.startsWith("actions/checkout@")) &&
        job.steps.some((step) => /\bgh release\b/.test(step.run ?? ""))
      )
      .map(([name]) => name)

    expect(ghReleaseJobs).toEqual([
      "assemble-draft",
      "upload-updater-metadata",
      "publish-release",
      "publish-beta-release",
    ])
    for (const name of ghReleaseJobs) {
      expect(workflow.jobs[name].env?.GH_REPO, name).toBe("${{ github.repository }}")
    }
  })

  it("normalizes raw REST release flags for the release state machine", () => {
    const resolve = releaseWorkflow().jobs.guard.steps.find(
      (step) => step.name === "Resolve release target"
    )

    expect(resolve?.run).toContain('RAW_RELEASE_JSON="$(sed -n')
    expect(resolve?.run).toContain('(.draft | type) == "boolean"')
    expect(resolve?.run).toContain('(.prerelease | type) == "boolean"')
    expect(resolve?.run).toContain("{isDraft: .draft, isPrerelease: .prerelease}")
  })

  it("synchronizes and verifies guard-approved notes before either channel publishes", () => {
    const workflow = releaseWorkflow()
    for (const [jobName, verifyStepName] of [
      ["publish-release", "Verify release assets and updater metadata"],
      ["publish-beta-release", "Verify beta release assets"],
    ]) {
      const job = workflow.jobs[jobName]
      const syncIndex = job.steps.findIndex(
        (step) => step.name === "Synchronize and verify release notes"
      )
      const verifyIndex = job.steps.findIndex((step) => step.name === verifyStepName)
      expect(syncIndex, jobName).toBeGreaterThanOrEqual(0)
      expect(syncIndex, jobName).toBeLessThan(verifyIndex)

      const sync = job.steps[syncIndex]
      expect(sync?.env?.RELEASE_NOTES_B64, jobName).toBe(
        "${{ needs.guard.outputs.release_notes_b64 }}"
      )
      expect(sync?.run, jobName).toContain(
        `printf '%s' "$RELEASE_NOTES_B64" | base64 --decode > release-notes.md`
      )
      expect(sync?.run, jobName).toContain(
        'gh release edit "$TAG_NAME" --notes-file release-notes.md'
      )
      expect(sync?.run, jobName).toContain(
        "gh release view \"$TAG_NAME\" --json body --jq '.body'"
      )
      expect(sync?.run, jobName).toContain(
        '[ "$ACTUAL_RELEASE_NOTES" = "$EXPECTED_RELEASE_NOTES" ]'
      )
    }
  })

  it("treats only an authoritative GitHub 404 as an absent release", () => {
    expect(apiLookupIsAuthoritativeNotFound(1, "HTTP/2.0 404 Not Found\n{}"))
      .toBe(true)
    for (const output of [
      "HTTP/2.0 401 Unauthorized\n{}",
      "HTTP/2.0 429 Too Many Requests\n{}",
      "HTTP/2.0 500 Internal Server Error\n{}",
      "network timeout",
    ]) {
      expect(apiLookupIsAuthoritativeNotFound(1, output)).toBe(false)
    }
  })

  it("rejects a verifier-stdout release-notes handoff", () => {
    const result = spawnSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow, verifyStableReleaseContract } from "./scripts/release-contract.ts";
        const source = await Bun.file(".github/workflows/release.yml").text();
        const workflow = parseReleaseWorkflow(source);
        const resolve = workflow.jobs.guard.steps.find((step) => step.name === "Resolve release target");
        resolve.run = resolve.run.replace(
          'RELEASE_NOTES_B64="$(base64 -w0 < "$RELEASE_NOTES_FILE")"',
          'RELEASE_NOTES_B64="$(bun scripts/release-notes.ts "$TAG_NAME" | base64 -w0)"'
        );
        verifyStableReleaseContract(workflow);
      `],
      { encoding: "utf8" }
    )

    expect(result.status).not.toBe(0)
    expect(result.stderr).toContain("encode the release-notes output file")
  })

  it("requires the generated numeric WiX override in stable, beta, and candidate builds", () => {
    const run = (contract: string, mutation: string) => spawnSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow, ${contract} } from "./scripts/release-contract.ts";
        const release = parseReleaseWorkflow(await Bun.file(".github/workflows/release.yml").text());
        const ci = parseReleaseWorkflow(await Bun.file(".github/workflows/ci.yml").text());
        ${mutation}
      `],
      { encoding: "utf8" }
    )

    const stable = run(
      "verifyStableReleaseContract",
      `release.jobs.build.steps.find((step) => step.name === "Build signed and notarized stable macOS installers").run = "bun tauri build --ci"; verifyStableReleaseContract(release);`
    )
    expect(stable.status).not.toBe(0)
    expect(stable.stderr).toContain("generated numeric WiX version override")

    const beta = run(
      "verifyBetaReleaseContract",
      `release.jobs.build.steps.find((step) => step.name === "Build signed and notarized beta macOS installers").run = "bun tauri build --ci"; verifyBetaReleaseContract(release, ci);`
    )
    expect(beta.status).not.toBe(0)
    expect(beta.stderr).toContain("generated no-updater numeric WiX version override")

    const candidate = run(
      "verifyBetaReleaseContract",
      `ci.jobs["release-candidate"].steps.find((step) => step.name === "Build unsigned release candidate").run = "bun tauri build --ci --no-sign"; verifyBetaReleaseContract(release, ci);`
    )
    expect(candidate.status).not.toBe(0)
    expect(candidate.stderr).toContain("release candidates must use the generated no-updater numeric WiX version override")
  })

  it("requires every released macOS installer to be Developer ID signed, notarized, and fail-closed verified", () => {
    const run = (contract: string, mutation: string) => spawnSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow, ${contract} } from "./scripts/release-contract.ts";
        const release = parseReleaseWorkflow(await Bun.file(".github/workflows/release.yml").text());
        const ci = parseReleaseWorkflow(await Bun.file(".github/workflows/ci.yml").text());
        ${mutation}
      `],
      { encoding: "utf8" }
    )

    const missingImport = run(
      "verifyStableReleaseContract",
      `release.jobs.build.steps.find((step) => step.name === "Import Developer ID Application certificate").run = "true"; verifyStableReleaseContract(release);`
    )
    expect(missingImport.status).not.toBe(0)
    expect(missingImport.stderr).toContain("import and verify a Developer ID Application")

    const unsignedBeta = run(
      "verifyBetaReleaseContract",
      `release.jobs.build.steps.find((step) => step.name === "Build signed and notarized beta macOS installers").run += " --no-sign"; verifyBetaReleaseContract(release, ci);`
    )
    expect(unsignedBeta.status).not.toBe(0)
    expect(unsignedBeta.stderr).toContain("without disabling Developer ID signing")

    const noGatekeeper = run(
      "verifyStableReleaseContract",
      `release.jobs.build.steps.find((step) => step.name === "Verify macOS Developer ID signature, Gatekeeper, and notarization").run = "codesign --verify --deep --strict app"; verifyStableReleaseContract(release);`
    )
    expect(noGatekeeper.status).not.toBe(0)
    expect(noGatekeeper.stderr).toContain("Gatekeeper, or stapling failure")
  })

  it("uses one bounded canonical-mirror installer for both Linux CI jobs", () => {
    const installer = readFileSync(".github/scripts/install-linux-system-dependencies.sh", "utf8")
    expect(installer).toContain("https://archive.ubuntu.com/ubuntu/")
    expect(installer).toContain("https://security.ubuntu.com/ubuntu/")
    expect(installer).toContain("Acquire::Retries=3")
    expect(installer).toContain("timeout --foreground 8m")
    expect(installer).toContain("timeout --foreground 12m")
  })

  it("rejects contents-write jobs that checkout or execute repository code", () => {
    const run = (mutation: string) => spawnSync(
      "bun",
      ["-e", `
        import { parseReleaseWorkflow, verifyStableReleaseContract } from "./scripts/release-contract.ts";
        const source = await Bun.file(".github/workflows/release.yml").text();
        const workflow = parseReleaseWorkflow(source);
        ${mutation}
        verifyStableReleaseContract(workflow);
      `],
      { encoding: "utf8" }
    )

    const checkoutWithWrite = run('workflow.jobs.build.permissions = { contents: "write" };')
    expect(checkoutWithWrite.status).not.toBe(0)
    expect(checkoutWithWrite.stderr).toContain("jobs.build must be contents: read")

    const writeJobRunsScript = run('workflow.jobs["publish-release"].steps.push({ name: "Unsafe repository script", run: "bun scripts/release-notes.ts v0.0.9-beta.1" });')
    expect(writeJobRunsScript.status).not.toBe(0)
    expect(writeJobRunsScript.stderr).toContain("repository scripts or build hooks")
  })

  it("accepts exactly one correct-version installer for each supported platform", () => {
    const installers = [
      "Yuzora_0.0.9-beta.1_universal.dmg",
      "Yuzora_0.0.9-beta.1_x64-setup.exe",
      "Yuzora_0.0.9-beta.1_x64_en-US.msi",
    ]
    expect(betaReleaseAssetNamesAreSafe(installers, "0.0.9-beta.1")).toBe(true)

    for (const unsafe of [
      [...installers, "latest.json"],
      [...installers, "Yuzora_0.0.9-beta.1_universal.dmg"],
      [
        "Yuzora_0.0.9-beta.10_universal.dmg",
        "Yuzora_0.0.9-beta.1_x64-setup.exe",
        "Yuzora_0.0.9-beta.1_x64_en-US.msi",
      ],
      [...installers, "Yuzora_0.0.9-beta.1_universal.dmg.SIG"],
      [...installers, "Yuzora_0.0.9-beta.1_universal.app.tar.gz"],
      [...installers, "Yuzora_0.0.9-beta.1_windows.zip"],
      [...installers, "Yuzora-windows-x64.msi"],
    ]) {
      expect(betaReleaseAssetNamesAreSafe(unsafe, "0.0.9-beta.1")).toBe(false)
    }
  })
})
