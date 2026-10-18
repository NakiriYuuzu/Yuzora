import { connect } from "./client"
import { takePairingLink } from "./lib/pairing"
import { navigate, storage, useCompanion } from "./store"

/** Picks the first screen. A pairing link wins and is stripped from the URL first. */
export function boot(): void {
  const link = takePairingLink(location, history)
  if (link) return navigate({ name: "pair", link })
  const binding = storage.binding()
  if (binding) {
    useCompanion.setState({ binding })
    navigate({ name: "home" })
    connect()
    return
  }
  navigate(storage.paired() ? { name: "login" } : { name: "pair", link: null })
}
