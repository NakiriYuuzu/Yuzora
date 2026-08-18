import {
  loadReleaseWorkflow,
  loadTauriConfig,
  verifyStableProductUpdaterConfig,
  verifyStableReleaseContract,
} from "./release-contract"

export async function verifyUpdaterReleaseContract() {
  verifyStableProductUpdaterConfig(await loadTauriConfig())
  verifyStableReleaseContract(await loadReleaseWorkflow())
  return "Stable updater release contract verified"
}

if (import.meta.main) {
  try {
    console.log(await verifyUpdaterReleaseContract())
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  }
}
