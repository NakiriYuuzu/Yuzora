import { connect } from "./client"
import { takePaneLink } from "./lib/deepLink"
import { takePairingLink } from "./lib/pairing"
import { navigate, storage, useCompanion } from "./store"

/**
 * Picks the first screen. A pairing link wins and is stripped from the URL
 * first. A notification's pane link is stripped too and waits for the first
 * snapshots, after a login if there is none yet.
 */
export function boot(): void {
  const link = takePairingLink(location, history)
  if (link) return navigate({ name: "pair", link })
  const pane = takePaneLink(location, history)
  if (pane) useCompanion.setState({ pendingPane: pane })
  const binding = storage.binding()
  if (binding) {
    useCompanion.setState({ binding })
    navigate({ name: "home" })
    connect()
    return
  }
  navigate(storage.paired() ? { name: "login" } : { name: "pair", link: null })
}
