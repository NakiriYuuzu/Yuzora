type UnknownRecord = Record<string, unknown>

type Workflow = UnknownRecord

// Verified upstream on 2026-08-26: actions/checkout releases/v4 backport #2524.
const REVIEWED_CHECKOUT_REF =
  "actions/checkout@11d5960a326750d5838078e36cf38b85af677262"

function record(value: unknown, label: string): UnknownRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as UnknownRecord
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

function steps(job: UnknownRecord, label: string): UnknownRecord[] {
  assert(Array.isArray(job.steps), `${label}.steps are required`)
  return job.steps.map((step, index) => record(step, `${label}.steps[${index}]`))
}

function stepByName(items: UnknownRecord[], name: string): UnknownRecord {
  const step = items.find((item) => item.name === name)
  assert(step, `missing workflow step: ${name}`)
  return step
}

function includes(value: unknown, text: string): boolean {
  return typeof value === "string" && value.includes(text)
}

function jobsFor(workflow: Workflow) {
  return record(workflow.jobs, "release workflow jobs")
}

function jobFor(workflow: Workflow, name: string) {
  return record(jobsFor(workflow)[name], `jobs.${name}`)
}

function guardFor(workflow: Workflow) {
  const guard = jobFor(workflow, "guard")
  return { guard, guardSteps: steps(guard, "jobs.guard") }
}

function contentsPermission(job: UnknownRecord, label: string) {
  return record(job.permissions, `${label}.permissions`).contents
}

function hasCheckout(job: UnknownRecord, label: string) {
  return steps(job, label).some(
    (step) => typeof step.uses === "string" && step.uses.startsWith("actions/checkout@")
  )
}

function hasRepositoryExecution(job: UnknownRecord, label: string) {
  return steps(job, label).some((step) => {
    if (typeof step.run !== "string") return false
    return /\b(?:bun|node|npm|pnpm|yarn)\b|\btauri\b|scripts\//.test(step.run)
  })
}

function hasGhReleaseCommand(job: UnknownRecord, label: string) {
  return steps(job, label).some(
    (step) => typeof step.run === "string" && /\bgh release\b/.test(step.run)
  )
}

export function parseReleaseWorkflow(text: string): Workflow {
  return record(Bun.YAML.parse(text), "release workflow")
}

export async function loadReleaseWorkflow(): Promise<Workflow> {
  return parseReleaseWorkflow(await Bun.file(".github/workflows/release.yml").text())
}

export async function loadCiWorkflow(): Promise<Workflow> {
  return record(Bun.YAML.parse(await Bun.file(".github/workflows/ci.yml").text()), "CI workflow")
}

export async function loadTauriConfig(): Promise<UnknownRecord> {
  return record(await Bun.file("src-tauri/tauri.conf.json").json(), "src-tauri/tauri.conf.json")
}

export function verifyStableProductUpdaterConfig(config: UnknownRecord): void {
  const bundle = record(config.bundle, "bundle")
  const plugins = record(config.plugins, "plugins")
  const updater = record(plugins.updater, "plugins.updater")
  assert(bundle.createUpdaterArtifacts === true, "bundle.createUpdaterArtifacts must remain true for stable")
  assert(bundle.targets === "all", "bundle.targets must keep MSI and NSIS manual assets")
  assert(typeof updater.pubkey === "string" && updater.pubkey.length > 0, "updater pubkey is required")
  assert(
    Array.isArray(updater.endpoints) &&
      updater.endpoints.includes("https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/latest.json"),
    "stable latest.json endpoint is required"
  )
}

export function betaReleaseAssetNamesAreSafe(assetNames: Iterable<string>, version: string): boolean {
  const names = [...assetNames]
  if (names.length !== 3) return false

  const prefix = `Yuzora_${version}_`
  const installers = { dmg: 0, setup: 0, msi: 0 }
  for (const name of names) {
    if (!name.startsWith(prefix)) return false
    const suffix = name.slice(prefix.length)
    if (!suffix) return false
    if (suffix.endsWith(".dmg")) installers.dmg += 1
    else if (suffix.endsWith("-setup.exe")) installers.setup += 1
    else if (suffix.endsWith(".msi")) installers.msi += 1
    else return false
  }
  return installers.dmg === 1 && installers.setup === 1 && installers.msi === 1
}

export function apiLookupIsAuthoritativeNotFound(exitCode: number, output: string): boolean {
  return exitCode !== 0 && /^HTTP\/[^ ]+ 404(?: |$)/m.test(output)
}

export function isPinnedReleaseActionRef(value: unknown): value is string {
  return typeof value === "string" && /^[^@\s]+@[a-f0-9]{40}$/.test(value)
}

function verifyReleaseActionPins(workflow: Workflow): void {
  for (const [jobName, rawJob] of Object.entries(jobsFor(workflow))) {
    const job = record(rawJob, `jobs.${jobName}`)
    for (const step of steps(job, `jobs.${jobName}`)) {
      if (step.uses === undefined) continue
      assert(
        isPinnedReleaseActionRef(step.uses),
        `jobs.${jobName} action must be pinned to a full 40-character commit SHA`
      )
      if (step.uses.startsWith("actions/checkout@")) {
        assert(
          step.uses === REVIEWED_CHECKOUT_REF,
          `jobs.${jobName} checkout must use the reviewed actions/checkout commit`
        )
        const withOptions = record(step.with, `jobs.${jobName} checkout.with`)
        assert(
          withOptions["persist-credentials"] === false,
          `jobs.${jobName} checkout must set persist-credentials: false`
        )
      }
    }
  }
}

function verifyReleaseLeastPrivilege(workflow: Workflow): void {
  const defaultPermissions = record(workflow.permissions, "release workflow permissions")
  assert(defaultPermissions.contents === "read", "release workflow default permissions must be contents: read")

  for (const name of ["guard", "build", "prepare-updater-metadata"]) {
    const job = jobFor(workflow, name)
    assert(contentsPermission(job, `jobs.${name}`) === "read", `jobs.${name} must be contents: read`)
    assert(hasCheckout(job, `jobs.${name}`), `jobs.${name} must explicitly checkout the verified source`)
  }

  for (const name of [
    "create-tag",
    "assemble-draft",
    "upload-updater-metadata",
    "publish-release",
    "publish-beta-release",
  ]) {
    const job = jobFor(workflow, name)
    assert(contentsPermission(job, `jobs.${name}`) === "write", `jobs.${name} must be contents: write`)
    assert(!hasCheckout(job, `jobs.${name}`), `jobs.${name} must not checkout repository code`)
    assert(
      !hasRepositoryExecution(job, `jobs.${name}`),
      `jobs.${name} must not execute repository scripts or build hooks with write authority`
    )
  }

  for (const [name, rawJob] of Object.entries(jobsFor(workflow))) {
    const job = record(rawJob, `jobs.${name}`)
    if (!hasCheckout(job, `jobs.${name}`) && hasGhReleaseCommand(job, `jobs.${name}`)) {
      const environment =
        job.env && typeof job.env === "object" && !Array.isArray(job.env)
          ? (job.env as UnknownRecord)
          : {}
      assert(
        environment.GH_REPO === "${{ github.repository }}",
        `jobs.${name} must set GH_REPO because gh release cannot rely on a no-checkout working directory`
      )
    }
    if (contentsPermission(job, `jobs.${name}`) !== "write") continue
    assert(
      !hasCheckout(job, `jobs.${name}`) && !hasRepositoryExecution(job, `jobs.${name}`),
      `jobs.${name} combines write authority with checked-out or repository-controlled code execution`
    )
  }

  const tag = stepByName(
    steps(jobFor(workflow, "create-tag"), "jobs.create-tag"),
    "Create annotated tag from verified main commit"
  )
  assert(
    includes(tag.run, 'gh api --method POST "repos/${GITHUB_REPOSITORY}/git/tags"') &&
      includes(tag.run, 'gh api --method POST "repos/${GITHUB_REPOSITORY}/git/refs"') &&
      !includes(tag.run, "git push") &&
      !includes(tag.run, "git config user"),
    "release tags must be created by a no-checkout GitHub API job"
  )
}

function verifyReleaseLookupFailureHandling(workflow: Workflow): void {
  const resolve = stepByName(guardFor(workflow).guardSteps, "Resolve release target")
  assert(
    includes(resolve.run, "api_response_is_404") &&
      includes(resolve.run, "HTTP/[^ ]+ 404") &&
      includes(resolve.run, "could not resolve GitHub Release state") &&
      includes(resolve.run, "could not resolve Git tag state"),
    "release lookup must treat only authoritative HTTP 404 as absent and fail all other API errors"
  )
}

function verifyReleaseStateNormalization(workflow: Workflow): void {
  const resolve = stepByName(guardFor(workflow).guardSteps, "Resolve release target")
  assert(
    includes(resolve.run, 'RAW_RELEASE_JSON="$(sed -n') &&
      includes(resolve.run, '(.draft | type) == "boolean"') &&
      includes(resolve.run, '(.prerelease | type) == "boolean"') &&
      includes(resolve.run, "{isDraft: .draft, isPrerelease: .prerelease}"),
    "guard must validate and normalize REST draft/prerelease flags for the release state machine"
  )
}

function verifyReleaseNotesHandoff(workflow: Workflow): void {
  const resolve = stepByName(guardFor(workflow).guardSteps, "Resolve release target")
  assert(
    includes(resolve.run, 'RELEASE_NOTES_FILE="$(mktemp)"') &&
      includes(resolve.run, 'bun scripts/release-notes.ts "$TAG_NAME" "$RELEASE_NOTES_FILE"') &&
      includes(resolve.run, 'RELEASE_NOTES_B64="$(base64 -w0 < "$RELEASE_NOTES_FILE")"') &&
      !includes(resolve.run, 'bun scripts/release-notes.ts "$TAG_NAME" | base64'),
    "guard must encode the release-notes output file, never verifier stdout"
  )

  const assemble = stepByName(
    steps(jobFor(workflow, "assemble-draft"), "jobs.assemble-draft"),
    "Create or repair draft and upload versioned assets"
  )
  assert(
    includes(assemble.run, "printf '%s' \"$RELEASE_NOTES_B64\" | base64 --decode > release-notes.md") &&
      includes(assemble.run, "--notes-file release-notes.md"),
    "draft assembly must decode the guard's release-notes bytes directly into its notes file"
  )

  for (const [jobName, verificationStepName] of [
    ["publish-release", "Verify release assets and updater metadata"],
    ["publish-beta-release", "Verify beta release assets"],
  ]) {
    const publishSteps = steps(jobFor(workflow, jobName), `jobs.${jobName}`)
    const syncNotes = stepByName(publishSteps, "Synchronize and verify release notes")
    const verifyRelease = stepByName(publishSteps, verificationStepName)
    const syncEnv = record(syncNotes.env, `jobs.${jobName} release notes env`)
    assert(
      publishSteps.indexOf(syncNotes) < publishSteps.indexOf(verifyRelease) &&
        syncEnv.RELEASE_NOTES_B64 === "${{ needs.guard.outputs.release_notes_b64 }}" &&
        includes(syncNotes.run, "printf '%s' \"$RELEASE_NOTES_B64\" | base64 --decode > release-notes.md") &&
        includes(syncNotes.run, 'gh release edit "$TAG_NAME" --notes-file release-notes.md') &&
        includes(syncNotes.run, 'gh release view "$TAG_NAME" --json body --jq \'.body\'') &&
        includes(syncNotes.run, '[ "$ACTUAL_RELEASE_NOTES" = "$EXPECTED_RELEASE_NOTES" ]'),
      `${jobName} must replace and verify the draft body with guard-approved release notes before publication`
    )
  }
}

function verifyArtifactBoundary(workflow: Workflow): void {
  const build = jobFor(workflow, "build")
  const buildSteps = steps(build, "jobs.build")
  assert(
    build.needs && JSON.stringify(build.needs).includes("create-tag"),
    "read-only build must wait for tag creation or a verified existing tag"
  )
  assert(
    buildSteps.some((step) => step.name === "Upload local release artifacts" && typeof step.uses === "string" && step.uses.startsWith("actions/upload-artifact@")),
    "read-only build must upload installers as Actions artifacts"
  )
  assert(
    !buildSteps.some((step) => typeof step.uses === "string" && step.uses.startsWith("tauri-apps/tauri-action@")),
    "build must not use tauri-action release upload mode"
  )
  const collect = stepByName(buildSteps, "Collect verified local installer artifacts")
  assert(
    includes(collect.run, "BUNDLE_DIR=\"src-tauri/target/universal-apple-darwin/release/bundle\"") &&
      includes(collect.run, "BUNDLE_DIR=\"src-tauri/target/release/bundle\"") &&
      includes(collect.run, "dmg/*.dmg") &&
      includes(collect.run, "macos/*.app.tar.gz") &&
      includes(collect.run, "nsis/*setup.exe") &&
      includes(collect.run, "msi/*.msi") &&
      includes(collect.run, "*.app.tar.gz.sig") &&
      includes(collect.run, "*.msi.sig"),
    "build must validate Tauri CLI macOS universal and Windows NSIS/MSI/updater output paths"
  )

  const assemble = jobFor(workflow, "assemble-draft")
  const assembleUpload = stepByName(
    steps(assemble, "jobs.assemble-draft"),
    "Create or repair draft and upload versioned assets"
  )
  const assembleEnv = record(assembleUpload.env, "jobs.assemble-draft upload env")
  assert(
    includes(assemble.if, "needs.guard.result == 'success'") &&
      includes(assemble.if, "needs.guard.outputs.should_build == 'true'") &&
      includes(assemble.if, "needs.build.result == 'success'") &&
      steps(assemble, "jobs.assemble-draft").some((step) => step.name === "Download validated installer artifacts"),
    "write-only assembly must require the guarded release decision and a successful read-only build"
  )
  assert(
    assembleEnv.RESUME_EXISTING_DRAFT ===
      "${{ needs.guard.outputs.should_publish_existing }}" &&
      includes(
        assembleUpload.run,
        'if RELEASE_JSON="$(gh release view "$TAG_NAME" --json isDraft,isPrerelease'
      ) &&
      includes(assembleUpload.run, "existing release must remain a draft") &&
      includes(assembleUpload.run, "existing draft channel changed after guard") &&
      includes(assembleUpload.run, 'if [ "$RESUME_EXISTING_DRAFT" = "true" ]') &&
      includes(assembleUpload.run, "guard-verified draft disappeared before assembly") &&
      includes(
        assembleUpload.run,
        'gh release edit "$TAG_NAME" --title "Yuzora ${TAG_NAME}" --notes-file release-notes.md'
      ) &&
      includes(assembleUpload.run, '[ "$ACTUAL_RELEASE_NOTES" = "$EXPECTED_RELEASE_NOTES" ]') &&
      includes(assembleUpload.run, 'gh release upload "$TAG_NAME" "${ASSETS[@]}" --clobber') &&
      includes(assembleUpload.run, 'gh release upload "$TAG_NAME" "$alias" --clobber'),
    "matching drafts must rebuild and idempotently repair notes, versioned assets, and stable aliases"
  )

  const prepare = jobFor(workflow, "prepare-updater-metadata")
  assert(
    contentsPermission(prepare, "jobs.prepare-updater-metadata") === "read" &&
      hasCheckout(prepare, "jobs.prepare-updater-metadata") &&
      includes(prepare.if, "needs.guard.outputs.should_build == 'true'") &&
      includes(prepare.if, "needs.build.result == 'success'") &&
      includes(prepare.if, "needs.assemble-draft.result == 'success'") &&
      !includes(prepare.if, "skipped") &&
      !includes(prepare.if, "should_publish_existing"),
    "read-only metadata preparation must require rebuilt and reassembled draft assets"
  )
  const prepareSteps = steps(prepare, "jobs.prepare-updater-metadata")
  assert(
    stepByName(prepareSteps, "Generate and validate stable updater metadata").run?.includes("prepare-updater-metadata.ts") &&
      prepareSteps.some((step) => step.name === "Upload finalized updater metadata for write-only publication"),
    "read-only metadata generation must hand off latest.json through an Actions artifact"
  )

  const upload = jobFor(workflow, "upload-updater-metadata")
  const uploadSteps = steps(upload, "jobs.upload-updater-metadata")
  assert(
    upload.needs && JSON.stringify(upload.needs).includes("prepare-updater-metadata") &&
      uploadSteps.some((step) => step.name === "Download finalized metadata") &&
      includes(stepByName(uploadSteps, "Replace metadata and remove unsupported draft assets").run, "gh release upload") &&
      includes(stepByName(uploadSteps, "Replace metadata and remove unsupported draft assets").run, "--clobber"),
    "no-checkout write-only job must upload finalized metadata"
  )
}

function verifyAppleNotarizationEnvironment(step: UnknownRecord, label: string): void {
  const env = record(step.env, `${label}.env`)
  for (const name of [
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
  ]) {
    assert(
      env[name] === `\${{ secrets.${name} }}`,
      `${label} must receive ${name} from protected secrets`
    )
  }
}

function verifyMacOsDistributionContract(workflow: Workflow): void {
  const buildSteps = steps(jobFor(workflow, "build"), "jobs.build")
  const importCertificate = stepByName(buildSteps, "Import Developer ID Application certificate")
  assert(
    importCertificate.if === "matrix.artifact_name == 'macos'",
    "Developer ID certificate import must run only on the macOS release runner"
  )
  const importEnv = record(importCertificate.env, "Developer ID certificate import env")
  for (const name of [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
  ]) {
    assert(
      importEnv[name] === `\${{ secrets.${name} }}`,
      `macOS release must fail closed unless ${name} is configured`
    )
  }
  assert(
    includes(importCertificate.run, "Developer ID Application:") &&
      includes(importCertificate.run, "openssl base64 -d -A") &&
      includes(importCertificate.run, "security create-keychain") &&
      includes(importCertificate.run, "security import") &&
      includes(importCertificate.run, "security set-key-partition-list") &&
      includes(importCertificate.run, "security find-identity"),
    "macOS release must import and verify a Developer ID Application .p12 in an isolated keychain"
  )

  const verifyDistribution = stepByName(
    buildSteps,
    "Verify macOS Developer ID signature, Gatekeeper, and notarization"
  )
  assert(
    verifyDistribution.if === "matrix.artifact_name == 'macos'",
    "macOS distribution verification must run only on the macOS release runner"
  )
  const verificationEnv = record(verifyDistribution.env, "macOS distribution verification env")
  assert(
    verificationEnv.APPLE_TEAM_ID === "${{ secrets.APPLE_TEAM_ID }}",
    "macOS distribution verification must compare the signed TeamIdentifier with APPLE_TEAM_ID"
  )
  assert(
    includes(verifyDistribution.run, "codesign --verify --deep --strict") &&
      includes(verifyDistribution.run, "Authority=Developer ID Application:") &&
      includes(verifyDistribution.run, "TeamIdentifier=${APPLE_TEAM_ID}") &&
      includes(verifyDistribution.run, "spctl --assess --type execute") &&
      includes(verifyDistribution.run, 'xcrun stapler validate "$APP_PATH"') &&
      includes(verifyDistribution.run, 'xcrun stapler validate "$DMG_PATH"'),
    "macOS release must fail closed on strict code-signing, Developer ID, Team ID, Gatekeeper, or stapling failure"
  )

  const cleanup = stepByName(buildSteps, "Remove temporary macOS signing keychain")
  assert(
    includes(cleanup.if, "always()") &&
      includes(cleanup.if, "matrix.artifact_name == 'macos'") &&
      includes(cleanup.run, "security delete-keychain"),
    "macOS signing credentials must be removed from the temporary runner keychain even after failure"
  )
}

function verifyReleaseWorkflowHardening(workflow: Workflow): void {
  verifyReleaseActionPins(workflow)
  verifyReleaseLeastPrivilege(workflow)
  verifyReleaseLookupFailureHandling(workflow)
  verifyReleaseStateNormalization(workflow)
  verifyReleaseNotesHandoff(workflow)
  verifyArtifactBoundary(workflow)
  verifyMacOsDistributionContract(workflow)
}

export function verifyStableReleaseContract(workflow: Workflow): void {
  verifyReleaseWorkflowHardening(workflow)
  const triggers = record(workflow.on, "release workflow triggers")
  const workflowRun = record(triggers.workflow_run, "workflow_run trigger")
  assert(
    Array.isArray(workflowRun.workflows) && workflowRun.workflows.length === 1 && workflowRun.workflows[0] === "CI",
    "release must follow the successful CI workflow"
  )
  assert(
    Array.isArray(workflowRun.branches) && workflowRun.branches.length === 1 && workflowRun.branches[0] === "main",
    "release workflow_run must only follow main"
  )

  const { guard, guardSteps } = guardFor(workflow)
  assert(
    includes(guard.if, "workflow_run.conclusion == 'success'") &&
      includes(guard.if, "workflow_run.event == 'push'") &&
      includes(guard.if, "workflow_run.head_branch == 'main'"),
    "release guard must only accept a successful main push CI run"
  )
  const outputs = record(guard.outputs, "guard outputs")
  assert(outputs.channel === "${{ steps.release.outputs.channel }}", "guard must expose channel")
  assert(outputs.is_beta === "${{ steps.release.outputs.is_beta }}", "guard must expose is_beta")
  assert(outputs.release_notes_b64 === "${{ steps.release.outputs.release_notes_b64 }}", "guard must expose release notes for no-checkout assembly")

  const signing = stepByName(guardSteps, "Validate stable updater signing inputs")
  assert(signing.if === "steps.release.outputs.is_beta != 'true'", "stable signing guard must exclude beta")
  const signingEnv = record(signing.env, "stable signing guard env")
  assert(
    signingEnv.TAURI_SIGNING_PRIVATE_KEY === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}" &&
      signingEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    "stable signing guard must read protected updater secrets"
  )

  const build = jobFor(workflow, "build")
  assert(
    includes(build.if, "needs.guard.outputs.should_build == 'true'"),
    "build must require a verified new or recoverable draft release attempt"
  )
  const matrix = record(record(build.strategy, "build strategy").matrix, "build matrix")
  assert(Array.isArray(matrix.include), "release build matrix is required")
  const platforms = matrix.include.map((row, index) => record(row, `build matrix row ${index}`).platform)
  assert(platforms.includes("macos-latest") && platforms.includes("windows-latest") && !platforms.includes("ubuntu-22.04"), "release must build only macOS and Windows installers")
  const buildSteps = steps(build, "jobs.build")
  const stableBuild = stepByName(buildSteps, "Build signed and notarized stable macOS installers")
  assert(
    includes(stableBuild.if, "needs.guard.outputs.is_beta != 'true'") &&
      includes(stableBuild.if, "matrix.artifact_name == 'macos'"),
    "stable macOS build must be platform- and channel-gated"
  )
  const stableEnv = record(stableBuild.env, "stable macOS build env")
  assert(
    stableEnv.TAURI_SIGNING_PRIVATE_KEY === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}" &&
      stableEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    "stable macOS build must receive updater signing secrets"
  )
  verifyAppleNotarizationEnvironment(stableBuild, "stable macOS build")
  assert(
    includes(stableBuild.run, "bun tauri build --ci") &&
      includes(stableBuild.run, 'scripts/release-msi-build-config.ts "$VERSION"') &&
      includes(stableBuild.run, '--config "$RELEASE_BUILD_CONFIG"') &&
      !includes(stableBuild.run, "--no-updater") &&
      !includes(stableBuild.run, "--no-sign"),
    "stable must use the generated numeric WiX version override without disabling signing or updater artifacts"
  )

  const stableWindowsBuild = stepByName(buildSteps, "Build stable Windows installers")
  assert(
    includes(stableWindowsBuild.if, "needs.guard.outputs.is_beta != 'true'") &&
      includes(stableWindowsBuild.if, "matrix.artifact_name == 'windows'") &&
      !JSON.stringify(stableWindowsBuild.env ?? {}).includes("APPLE_"),
    "stable Windows build must remain separate from Apple signing credentials"
  )

  const publish = jobFor(workflow, "publish-release")
  assert(
    includes(publish.if, "needs.guard.outputs.is_beta != 'true'") &&
      includes(publish.if, "needs.upload-updater-metadata.result == 'success'"),
    "stable publish must require final updater metadata upload"
  )
  const publishStep = stepByName(steps(publish, "jobs.publish-release"), "Publish verified stable release")
  assert(
    includes(publishStep.run, "--draft=false") &&
      includes(publishStep.run, "--prerelease=false") &&
      includes(publishStep.run, "--latest"),
    "stable publish must remain Latest and non-prerelease"
  )
}

export function verifyBetaReleaseContract(workflow: Workflow, ci: Workflow): void {
  verifyReleaseWorkflowHardening(workflow)
  const { guardSteps } = guardFor(workflow)
  const resolve = stepByName(guardSteps, "Resolve release target")
  assert(
    includes(resolve.run, "CHANNEL=\"$(bun -e") &&
      includes(resolve.run, "YUZORA_RELEASE_STATE") &&
      includes(resolve.run, "bun scripts/release-state.ts"),
    "guard must classify releases through the tested release state machine"
  )
  const betaContract = stepByName(guardSteps, "Verify beta prerelease contract")
  assert(betaContract.if === "steps.release.outputs.is_beta == 'true'", "beta contract must only run for beta")
  assert(betaContract.run === "bun run check:beta-release", "beta contract command changed")

  const build = jobFor(workflow, "build")
  const buildSteps = steps(build, "jobs.build")
  const betaBuild = stepByName(buildSteps, "Build signed and notarized beta macOS installers")
  assert(
    includes(betaBuild.if, "needs.guard.outputs.is_beta == 'true'") &&
      includes(betaBuild.if, "matrix.artifact_name == 'macos'"),
    "beta macOS build must be platform- and channel-gated"
  )
  verifyAppleNotarizationEnvironment(betaBuild, "beta macOS build")
  assert(
    !JSON.stringify(betaBuild.env ?? {}).includes("TAURI_SIGNING_PRIVATE_KEY") &&
      !JSON.stringify(betaBuild.env ?? {}).includes("GITHUB_TOKEN") &&
      !includes(betaBuild.run, "--no-sign") &&
      includes(betaBuild.run, 'scripts/release-msi-build-config.ts "$VERSION" --no-updater') &&
      includes(betaBuild.run, '--config "$RELEASE_BUILD_CONFIG"'),
    "beta macOS build must use the generated no-updater numeric WiX version override without disabling Developer ID signing"
  )

  const betaWindowsBuild = stepByName(buildSteps, "Build unsigned beta Windows installers")
  assert(
    includes(betaWindowsBuild.if, "needs.guard.outputs.is_beta == 'true'") &&
      includes(betaWindowsBuild.if, "matrix.artifact_name == 'windows'") &&
      !JSON.stringify(betaWindowsBuild.env ?? {}).includes("TAURI_SIGNING_PRIVATE_KEY") &&
      !JSON.stringify(betaWindowsBuild.env ?? {}).includes("APPLE_") &&
      includes(betaWindowsBuild.run, "--no-sign") &&
      includes(betaWindowsBuild.run, 'scripts/release-msi-build-config.ts "$VERSION" --no-updater') &&
      includes(betaWindowsBuild.run, '--config "$RELEASE_BUILD_CONFIG"'),
    "beta Windows build must use the no-updater numeric WiX override without updater or Apple secrets"
  )

  const betaPublish = jobFor(workflow, "publish-beta-release")
  assert(
    includes(betaPublish.if, "needs.guard.outputs.is_beta == 'true'") &&
      includes(betaPublish.if, "needs.guard.outputs.should_build == 'true'") &&
      includes(betaPublish.if, "needs.assemble-draft.result == 'success'") &&
      !includes(betaPublish.if, "skipped") &&
      !includes(betaPublish.if, "should_publish_existing"),
    "beta publish must require a rebuilt and successfully reassembled draft"
  )
  const betaSteps = steps(betaPublish, "jobs.publish-beta-release")
  const verify = stepByName(betaSteps, "Verify beta release assets")
  assert(
    includes(verify.run, "ASSET_COUNT") &&
      includes(verify.run, "unexpected beta asset") &&
      includes(verify.run, "case \"$asset\" in"),
    "beta publish must use an exact installer-only allowlist"
  )
  const publish = stepByName(betaSteps, "Publish verified beta prerelease")
  assert(
    includes(publish.run, "--draft=false") &&
      includes(publish.run, "--prerelease=true") &&
      !includes(publish.run, "--latest"),
    "beta publish must be prerelease-only and never Latest"
  )

  const candidate = record(record(ci.jobs, "CI jobs")["release-candidate"], "release candidate")
  assert(
    includes(candidate.if, "startsWith(github.head_ref, 'release/')") &&
      includes(candidate.if, "github.event_name == 'pull_request'"),
    "release candidates must run only for release pull requests"
  )
  const branchCheck = stepByName(steps(candidate, "release candidate"), "Verify release candidate branch matches product version")
  assert(
    includes(branchCheck.run, '"$HEAD_REF" != "release/v${VERSION}"'),
    "candidate installers must require the exact release/v<product-version> branch"
  )
  const candidateBuild = stepByName(steps(candidate, "release candidate"), "Build unsigned release candidate")
  assert(
    includes(candidateBuild.run, 'scripts/release-msi-build-config.ts "$VERSION" --no-updater') &&
      includes(candidateBuild.run, '--config "$RELEASE_BUILD_CONFIG"') &&
      includes(candidateBuild.run, "--no-sign"),
    "release candidates must use the generated no-updater numeric WiX version override for every channel"
  )

  verifyCiLinuxDependencySetup(ci)
}

function verifyCiLinuxDependencySetup(ci: Workflow): void {
  const jobs = jobsFor(ci)
  for (const [jobName, stepName] of [
    ["rust-compile", "Install Linux system dependencies"],
    ["database-integration", "Install Linux system dependencies"],
  ]) {
    const install = stepByName(steps(record(jobs[jobName], `CI jobs.${jobName}`), `CI jobs.${jobName}`), stepName)
    assert(
      install.run === "sudo bash .github/scripts/install-linux-system-dependencies.sh",
      `${jobName} must use the bounded canonical-mirror Linux dependency installer`
    )
  }
}
