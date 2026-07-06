import { afterEach, describe, expect, it } from "vitest"

import { buildXtermTheme } from "./xtermTheme"

const tokenValues = {
    "--term-bg": "#101010",
    "--term-bar": "#202020",
    "--term-fg": "#f0f0f0",
    "--term-fg2": "#999999",
    "--term-line": "rgba(255, 255, 255, 0.12)",
    "--term-chip": "rgba(255, 255, 255, 0.16)",
    "--term-hover": "rgba(255, 255, 255, 0.2)",
    "--term-green": "#33cc88",
    "--term-blue": "#6699ff",
    "--term-lime": "#bbdd55",
    "--term-coral": "#ff7755",
    "--term-ok": "#44dd99",
    "--term-amber": "#ddaa44"
}

function setTerminalTokens(values: Record<string, string>) {
    for (const [name, value] of Object.entries(values)) {
        document.documentElement.style.setProperty(name, value)
    }
}

afterEach(() => {
    for (const name of Object.keys(tokenValues)) {
        document.documentElement.style.removeProperty(name)
    }
})

describe("buildXtermTheme", () => {
    it("maps terminal CSS variables into xterm chrome colors at call time", () => {
        setTerminalTokens(tokenValues)

        const theme = buildXtermTheme("dark")

        expect(theme.background).toBe("#101010")
        expect(theme.foreground).toBe("#f0f0f0")
        expect(theme.cursor).toBe("#33cc88")
        expect(theme.cursorAccent).toBe("#101010")
        expect(theme.selectionBackground).toBe("rgba(255, 255, 255, 0.16)")
        expect(theme.selectionForeground).toBe("#f0f0f0")
        expect(theme.selectionInactiveBackground).toBe("rgba(255, 255, 255, 0.2)")
        expect(theme.scrollbarSliderBackground).toBe("rgba(255, 255, 255, 0.16)")
        expect(theme.scrollbarSliderHoverBackground).toBe("rgba(255, 255, 255, 0.2)")
        expect(theme.scrollbarSliderActiveBackground).toBe("rgba(255, 255, 255, 0.12)")
        expect(theme.overviewRulerBorder).toBe("rgba(255, 255, 255, 0.12)")
    })

    it("includes hardcoded ANSI 16 palettes that differ between light and dark modes", () => {
        setTerminalTokens(tokenValues)

        const light = buildXtermTheme("light")
        const dark = buildXtermTheme("dark")

        expect(light.black).toBe("#5c5a55")
        expect(light.red).toBe("#b43d3d")
        expect(light.green).toBe("#2f8f5f")
        expect(light.yellow).toBe("#a8690f")
        expect(light.blue).toBe("#2456cc")
        expect(light.magenta).toBe("#8a4dbf")
        expect(light.cyan).toBe("#1f7f8a")
        expect(light.white).toBe("#f7f3ea")
        expect(light.brightBlack).toBe("#8a8691")
        expect(light.brightRed).toBe("#d65f5f")
        expect(light.brightGreen).toBe("#42a870")
        expect(light.brightYellow).toBe("#c4841c")
        expect(light.brightBlue).toBe("#3d6df0")
        expect(light.brightMagenta).toBe("#a86bd6")
        expect(light.brightCyan).toBe("#3198a3")
        expect(light.brightWhite).toBe("#ffffff")

        expect(dark.black).toBe("#0f0e13")
        expect(dark.red).toBe("#ff6b6b")
        expect(dark.green).toBe("#74d6a0")
        expect(dark.yellow).toBe("#e0b06a")
        expect(dark.blue).toBe("#82b4ff")
        expect(dark.magenta).toBe("#c792ea")
        expect(dark.cyan).toBe("#6bd3e6")
        expect(dark.white).toBe("#d6d3db")
        expect(dark.brightBlack).toBe("#847f8b")
        expect(dark.brightRed).toBe("#ff8f8f")
        expect(dark.brightGreen).toBe("#9be8bb")
        expect(dark.brightYellow).toBe("#f0c987")
        expect(dark.brightBlue).toBe("#a8caff")
        expect(dark.brightMagenta).toBe("#d8b0f2")
        expect(dark.brightCyan).toBe("#91e2f0")
        expect(dark.brightWhite).toBe("#ffffff")

        expect(light.red).not.toBe(dark.red)
        expect(light.blue).not.toBe(dark.blue)
    })
})
