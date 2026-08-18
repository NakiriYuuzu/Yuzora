import {
  loadCiWorkflow,
  loadReleaseWorkflow,
  verifyBetaReleaseContract,
} from "./release-contract"

export async function verifyBetaReleaseContractFile() {
  verifyBetaReleaseContract(await loadReleaseWorkflow(), await loadCiWorkflow())
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
