/**
 * Window event dispatched after the local HERDR binary source was saved
 * (custom path or back to bundled). MachinesBridge listens and re-reads the
 * machines capabilities. Kept outside herdrIpc so tests mocking it still see it.
 */
export const HERDR_BINARY_SOURCE_CHANGED_EVENT = "yuzora:herdr-binary-source-changed"

export function notifyHerdrBinarySourceChanged(): void {
  window.dispatchEvent(new Event(HERDR_BINARY_SOURCE_CHANGED_EVENT))
}
