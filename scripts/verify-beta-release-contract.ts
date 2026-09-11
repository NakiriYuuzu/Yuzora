import {
  loadCiWorkflow,
  loadReleaseWorkflow,
  verifyBetaReleaseContract,
} from "./release-contract"
import { releaseMsiBuildConfig } from "./release-msi-build-config"

export async function verifyBetaReleaseContractFile() {
  verifyBetaReleaseContract(await loadReleaseWorkflow(), await loadCiWorkflow())
  const candidateConfig = releaseMsiBuildConfig("0.0.1-beta.1", true)
  if (candidateConfig.plugins?.updater.endpoints.length !== 0) {
    throw new Error("candidate builds must clear updater endpoints")
  }
  const betaConfig = releaseMsiBuildConfig("0.0.1-beta.1")
  if (betaConfig.bundle.createUpdaterArtifacts === false || betaConfig.plugins?.updater.endpoints.length === 0) {
    throw new Error("published beta builds must preserve updater artifacts and endpoints")
  }
  return "Beta prerelease contract verified"
}

if (import.meta.main) {
  try {
    console.log(await verifyBetaReleaseContractFile())
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  }
}
