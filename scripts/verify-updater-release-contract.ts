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
const jobs = record(workflow.jobs, "jobs")
const guard = record(jobs.guard, "jobs.guard")
const build = record(jobs.build, "jobs.build")
const finalize = record(jobs["finalize-updater-metadata"], "jobs.finalize-updater-metadata")
const guardSteps = guard.steps
const buildSteps = build.steps
const finalizeSteps = finalize.steps
assert(Array.isArray(guardSteps), "guard steps are required")
assert(Array.isArray(buildSteps), "build steps are required")
assert(Array.isArray(finalizeSteps), "updater metadata finalizer steps are required")

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
  releaseNotesGuard.run === 'bun scripts/release-notes.ts "$GITHUB_REF_NAME"',
  "guard must validate the tagged CHANGELOG.md section"
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
      'bun scripts/release-notes.ts "$GITHUB_REF_NAME" release-notes.md'
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
assert(actionInputs.updaterJsonPreferNsis === false, "Windows updater metadata must prefer MSI")
assert(actionInputs.releaseDraft === true, "release must remain draft")
assert(actionInputs.prerelease === false, "stable release must not be a prerelease")
assert(actionInputs.tauriScript === "bun tauri", "release build must use the workspace Tauri CLI")

assert(finalize.needs === "build", "updater metadata must be finalized after every platform build")
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

console.log("Updater release contract verified: signed draft, stable latest.json, MSI-only OTA")
