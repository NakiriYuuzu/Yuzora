/** A pane a notification points at. */
export type PaneTarget = { session: string; pane: string }

// The gateway holds session and pane ids to 256 bytes.
const MAX_ID = 256

function target(session: unknown, pane: unknown): PaneTarget | null {
  const valid = (id: unknown): id is string => typeof id === "string" && id.length > 0 && id.length <= MAX_ID
  return valid(session) && valid(pane) ? { session, pane } : null
}

/**
 * Reads `#pane=<JSON [session, pane]>`, the link a notification opens a closed
 * app with, and strips it from the address bar. A malformed link is ignored.
 */
export function takePaneLink(location: Pick<Location, "hash">, history: Pick<History, "replaceState">): PaneTarget | null {
  const { hash } = location
  if (!hash.startsWith("#pane=")) return null
  history.replaceState(null, "", "/")
  try {
    const ids: unknown = JSON.parse(decodeURIComponent(hash.slice("#pane=".length)))
    return Array.isArray(ids) && ids.length === 2 ? target(ids[0], ids[1]) : null
  } catch {
    return null
  }
}

/** The service worker's `yzc-open-pane` message, which brings a running app to a pane. */
export function paneFromMessage(data: unknown): PaneTarget | null {
  if (typeof data !== "object" || data === null) return null
  const message = data as Record<string, unknown>
  return message.type === "yzc-open-pane" ? target(message.session, message.pane) : null
}
