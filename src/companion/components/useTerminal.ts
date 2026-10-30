import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Terminal } from "@xterm/xterm"
import { buildXtermTheme, xtermMinimumContrastRatio } from "@yuzora/shared/terminal/xtermTheme"

const FONT_SIZE = 13
const FONT_FAMILY = "'JetBrains Mono Variable', ui-monospace, Menlo, monospace"

/**
 * A read-only xterm at the size the gateway reports. Observe frames are fixed
 * to that size, so the phone scales the whole grid to its width instead of
 * reflowing; `actual` shows it 1:1 with horizontal scrolling, and the
 * browser's own pinch zoom stays available.
 */
export function useTerminal(): { terminal: Terminal | null; mount: (element: HTMLDivElement | null) => void } {
  const { i18n } = useTranslation()
  const [terminal, setTerminal] = useState<Terminal | null>(null)
  const [element, setElement] = useState<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!element) return
    const mode = document.documentElement.classList.contains("dark") ? "dark" : "light"
    const instance = new Terminal({
      disableStdin: true,
      cursorBlink: false,
      fontSize: FONT_SIZE,
      fontFamily: FONT_FAMILY,
      scrollback: 1000,
      theme: buildXtermTheme(mode),
      minimumContrastRatio: xtermMinimumContrastRatio(mode),
      // OSC 8 links open only after an explicit confirmation, http(s) only.
      linkHandler: {
        allowNonHttpProtocols: false,
        activate(_event, uri) {
          if (/^https?:\/\//i.test(uri) && window.confirm(i18n.t("pane.openLink", { url: uri }))) {
            window.open(uri, "_blank", "noopener,noreferrer")
          }
        },
      },
    })
    let disposed = false
    // Metrics come from the web font; measure only once it is loaded.
    void document.fonts.load(`${FONT_SIZE}px 'JetBrains Mono Variable'`).catch(() => undefined).then(() => {
      if (disposed) return
      instance.open(element)
      setTerminal(instance)
    })
    return () => {
      disposed = true
      setTerminal(null)
      instance.dispose()
    }
  }, [element, i18n])

  return { terminal, mount: setElement }
}
