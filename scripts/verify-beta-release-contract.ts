import {
  loadCiWorkflow,
  loadReleaseWorkflow,
  verifyBetaReleaseContract,
} from "./release-contract"
import { releaseMsiBuildConfig } from "./release-msi-build-config"

export async function verifyBetaReleaseContractFile() {
  verifyBetaReleaseContract(await loadReleaseWorkflow(), await loadCiWorkflow())
  const noUpdaterConfig = releaseMsiBuildConfig("0.0.1-beta.1", true)
  if (noUpdaterConfig.plugins?.updater.endpoints.length !== 0) {
    throw new Error("beta and candidate builds must clear updater endpoints")
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
