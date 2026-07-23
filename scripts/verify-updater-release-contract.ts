type UnknownRecord = Record<string, unknown>

function record(value: unknown, label: string): UnknownRecord {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`)
  }
  return value as UnknownRecord
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message)
}

const tauriConfig = record(
  await Bun.file("src-tauri/tauri.conf.json").json(),
  "src-tauri/tauri.conf.json"
)
const bundle = record(tauriConfig.bundle, "bundle")
const plugins = record(tauriConfig.plugins, "plugins")
const updater = record(plugins.updater, "plugins.updater")

assert(bundle.createUpdaterArtifacts === true, "bundle.createUpdaterArtifacts must be true")
assert(bundle.targets === "all", "bundle.targets must keep MSI and NSIS manual assets")
assert(typeof updater.pubkey === "string" && updater.pubkey.length > 0, "updater pubkey is required")
assert(
  Array.isArray(updater.endpoints) &&
    updater.endpoints.includes(
      "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/latest.json"
    ),
  "stable latest.json endpoint is required"
)

const workflow = record(
  Bun.YAML.parse(await Bun.file(".github/workflows/release.yml").text()),
  "release workflow"
)
const workflowTriggers = record(workflow.on, "release workflow triggers")
const workflowRunTrigger = record(workflowTriggers.workflow_run, "workflow_run trigger")
assert(
  Array.isArray(workflowRunTrigger.workflows) &&
    workflowRunTrigger.workflows.length === 1 &&
    workflowRunTrigger.workflows[0] === "CI",
  "release must follow the successful main CI workflow"
)
assert(
  Array.isArray(workflowRunTrigger.types) &&
    workflowRunTrigger.types.length === 1 &&
    workflowRunTrigger.types[0] === "completed",
  "release must wait for CI completion"
)
assert(
  Array.isArray(workflowRunTrigger.branches) &&
    workflowRunTrigger.branches.length === 1 &&
    workflowRunTrigger.branches[0] === "main",
  "release workflow_run must only follow the main branch"
)
const jobs = record(workflow.jobs, "jobs")
const guard = record(jobs.guard, "jobs.guard")
const build = record(jobs.build, "jobs.build")
const finalize = record(jobs["finalize-updater-metadata"], "jobs.finalize-updater-metadata")
const publish = record(jobs["publish-release"], "jobs.publish-release")
const guardSteps = guard.steps
const buildSteps = build.steps
const finalizeSteps = finalize.steps
const publishSteps = publish.steps
assert(Array.isArray(guardSteps), "guard steps are required")
assert(Array.isArray(buildSteps), "build steps are required")
assert(Array.isArray(finalizeSteps), "updater metadata finalizer steps are required")
assert(Array.isArray(publishSteps), "automated release publish steps are required")
assert(
  typeof guard.if === "string" &&
    guard.if.includes("workflow_run.conclusion == 'success'") &&
    guard.if.includes("workflow_run.event == 'push'") &&
    guard.if.includes("workflow_run.head_branch == 'main'"),
  "release guard must only accept a successful main push CI run"
)

const guardCheckout = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.uses === "actions/checkout@v4")
assert(guardCheckout, "guard must check out the verified main CI commit")
const guardCheckoutInputs = record(guardCheckout.with, "guard checkout inputs")
assert(
  guardCheckoutInputs.ref === "${{ github.event.workflow_run.head_sha }}",
  "guard must check out the exact successful main CI SHA"
)

const versionGuard = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.name === "Verify tag and product versions")
assert(versionGuard, "guard must verify the release tag against product versions")
assert(
  typeof versionGuard.run === "string" &&
    versionGuard.run.includes('GITHUB_REF_NAME="${{ steps.release.outputs.tag_name }}"') &&
    versionGuard.run.includes("bun run check:version"),
  "version guard must pass the resolved tag directly to the version check"
)

const signingGuard = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.name === "Validate updater signing inputs")
assert(signingGuard, "guard must validate updater signing inputs")
const signingGuardEnv = record(signingGuard.env, "signing guard env")
assert(
  signingGuardEnv.TAURI_SIGNING_PRIVATE_KEY === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
  "private key must come from the protected GitHub secret"
)
assert(
  signingGuardEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ===
    "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
  "private key password must come from the protected GitHub secret"
)

const releaseNotesGuard = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.name === "Verify user-facing release notes")
assert(releaseNotesGuard, "guard must require release notes for the tagged version")
assert(
  releaseNotesGuard.run === 'bun scripts/release-notes.ts "${{ steps.release.outputs.tag_name }}"',
  "guard must validate the tagged CHANGELOG.md section"
)

const resolveRelease = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.name === "Resolve release target")
assert(resolveRelease, "guard must resolve the version tag from the verified main commit")
assert(resolveRelease.id === "release", "release target step id must be release")
assert(
  typeof resolveRelease.run === "string" &&
    resolveRelease.run.includes("VERSION=\"$(jq -er '.version' package.json)\"") &&
    resolveRelease.run.includes("SHOULD_PUBLISH_EXISTING=true"),
  "release target must derive the tag and resume a validated existing draft"
)

const createTag = guardSteps
  .map((step, index) => record(step, `guard.steps[${index}]`))
  .find((step) => step.name === "Create release tag from verified main commit")
assert(createTag, "guard must automatically create the release tag")
assert(
  typeof createTag.run === "string" &&
    createTag.run.includes('git tag -a "$TAG_NAME" "$SOURCE_SHA"') &&
    createTag.run.includes('git push origin "refs/tags/${TAG_NAME}"'),
  "release tag must point to and push the exact successful main CI SHA"
)

assert(build.needs === "guard", "release build must wait for the main CI/version guard")
assert(
  typeof build.if === "string" && build.if.includes("needs.guard.outputs.should_build == 'true'"),
  "release build must only run for a new verified version"
)

const strategy = record(build.strategy, "build.strategy")
const matrix = record(strategy.matrix, "build.strategy.matrix")
assert(Array.isArray(matrix.include), "release matrix include is required")
const matrixRows = matrix.include.map((row, index) => record(row, `matrix.include[${index}]`))
assert(
  matrixRows.some(
    (row) => row.platform === "macos-latest" && row.args === "--target universal-apple-darwin"
  ),
  "macOS universal build is required"
)
assert(
  matrixRows.some((row) => row.platform === "ubuntu-22.04"),
  "Linux x86_64 build is required"
)
assert(
  matrixRows.some((row) => row.platform === "windows-latest"),
  "Windows x64 build is required"
)

const tauriAction = buildSteps
  .map((step, index) => record(step, `build.steps[${index}]`))
  .find((step) =>
    typeof step.uses === "string" ? step.uses.startsWith("tauri-apps/tauri-action@") : false
  )
assert(tauriAction, "tauri-action build step is required")
const releaseNotesStep = buildSteps
  .map((step, index) => record(step, `build.steps[${index}]`))
  .find((step) => step.name === "Extract user-facing release notes")
assert(releaseNotesStep, "build must extract release notes from CHANGELOG.md")
assert(releaseNotesStep.id === "release-notes", "release notes step id must be release-notes")
assert(
  typeof releaseNotesStep.run === "string" &&
    releaseNotesStep.run.includes(
      'bun scripts/release-notes.ts "${{ needs.guard.outputs.tag_name }}" release-notes.md'
    ) &&
    releaseNotesStep.run.includes("$GITHUB_OUTPUT"),
  "release notes step must expose the tagged CHANGELOG.md section"
)
const actionEnv = record(tauriAction.env, "tauri action env")
const actionInputs = record(tauriAction.with, "tauri action inputs")
assert(
  actionEnv.TAURI_SIGNING_PRIVATE_KEY === "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
  "build must receive the updater private key from GitHub secrets"
)
assert(
  actionEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ===
    "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
  "build must receive the updater key password from GitHub secrets"
)
assert(actionInputs.includeUpdaterJson === true, "tauri-action must upload latest.json")
assert(
  actionInputs.releaseBody === "${{ steps.release-notes.outputs.body }}",
  "GitHub Release and latest.json notes must use the tagged CHANGELOG.md section"
)
assert(
  actionInputs.tagName === "${{ needs.guard.outputs.tag_name }}",
  "tauri-action must build the tag resolved from the verified main CI commit"
)
assert(actionInputs.updaterJsonPreferNsis === false, "Windows updater metadata must prefer MSI")
assert(
  actionInputs.releaseDraft === true,
  "matrix builds must use a transient draft until every automated gate passes"
)
assert(actionInputs.prerelease === false, "stable release must not be a prerelease")
assert(actionInputs.tauriScript === "bun tauri", "release build must use the workspace Tauri CLI")

assert(
  Array.isArray(finalize.needs) &&
    finalize.needs.includes("guard") &&
    finalize.needs.includes("build"),
  "updater metadata must be finalized after every platform build"
)
const normalizedFinalizeSteps = finalizeSteps.map((step, index) =>
  record(step, `finalize.steps[${index}]`)
)
const sanitizeStep = normalizedFinalizeSteps.find(
  (step) => step.name === "Enforce MSI-only Windows updater metadata"
)
const replaceStep = normalizedFinalizeSteps.find(
  (step) => step.name === "Replace generated updater metadata"
)
assert(sanitizeStep, "release must remove NSIS entries from updater metadata")
assert(
  sanitizeStep.run === "bun scripts/finalize-updater-metadata.ts updater-release/latest.json",
  "release must run the checked-in updater metadata finalizer"
)
assert(replaceStep, "release must replace the generated updater metadata")
assert(
  typeof replaceStep.run === "string" && replaceStep.run.includes("--clobber"),
  "release must upload finalized latest.json with clobber"
)

const normalizedPublishSteps = publishSteps.map((step, index) =>
  record(step, `publish.steps[${index}]`)
)
const verifyPublish = normalizedPublishSteps.find(
  (step) => step.name === "Verify release assets and updater metadata"
)
const publishRelease = normalizedPublishSteps.find(
  (step) => step.name === "Publish verified release"
)
const confirmPublication = normalizedPublishSteps.find(
  (step) => step.name === "Confirm publication"
)
assert(verifyPublish, "automated publish must verify release assets and updater metadata")
assert(
  typeof verifyPublish.run === "string" &&
    verifyPublish.run.includes("darwin-aarch64 darwin-x86_64 linux-x86_64 windows-x86_64") &&
    verifyPublish.run.includes('endswith(".msi")'),
  "automated publish must require every updater platform and MSI-only Windows OTA"
)
assert(publishRelease, "verified releases must be published automatically")
assert(
  typeof publishRelease.run === "string" &&
    publishRelease.run.includes('gh release edit "$TAG_NAME"') &&
    publishRelease.run.includes("--draft=false"),
  "automated publish must remove draft status"
)
assert(confirmPublication, "automated publish must confirm GitHub publication state")

const ciWorkflow = record(
  Bun.YAML.parse(await Bun.file(".github/workflows/ci.yml").text()),
  "CI workflow"
)
const ciJobs = record(ciWorkflow.jobs, "CI jobs")
const releaseCandidate = record(ciJobs["release-candidate"], "CI release candidate")
assert(
  typeof releaseCandidate.if === "string" &&
    releaseCandidate.if.includes("github.event_name == 'pull_request'") &&
    releaseCandidate.if.includes("startsWith(github.head_ref, 'release/')"),
  "release candidate installers must only build for release pull requests"
)
const candidateStrategy = record(releaseCandidate.strategy, "release candidate strategy")
const candidateMatrix = record(candidateStrategy.matrix, "release candidate matrix")
assert(Array.isArray(candidateMatrix.include), "release candidate matrix include is required")
const candidateRows = candidateMatrix.include.map((row, index) =>
  record(row, `release candidate matrix row ${index}`)
)
for (const os of ["macos-latest", "windows-latest", "ubuntu-22.04"]) {
  assert(
    candidateRows.some((row) => row.os === os),
    `release candidate matrix must include ${os}`
  )
}
const candidateSteps = releaseCandidate.steps
assert(Array.isArray(candidateSteps), "release candidate steps are required")
const candidateBuild = candidateSteps
  .map((step, index) => record(step, `release candidate step ${index}`))
  .find((step) => step.name === "Build unsigned release candidate")
const candidateUpload = candidateSteps
  .map((step, index) => record(step, `release candidate step ${index}`))
  .find((step) => step.name === "Upload release candidate")
assert(candidateBuild, "release PRs must build runnable candidate installers")
assert(
  typeof candidateBuild.run === "string" &&
    candidateBuild.run.includes("bun tauri build --ci --no-sign") &&
    candidateBuild.run.includes('"createUpdaterArtifacts":false'),
  "release candidates must not create signed updater or Release artifacts"
)
assert(candidateUpload, "release PRs must upload candidate installers for user validation")
assert(
  candidateUpload.uses === "actions/upload-artifact@v4",
  "release candidates must use GitHub Actions artifacts"
)

console.log(
  "Updater release contract verified: user-tested PR candidates, auto tag, signed assets, MSI-only OTA, automated publish"
)
