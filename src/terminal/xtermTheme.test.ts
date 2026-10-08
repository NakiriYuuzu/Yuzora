import { afterEach, describe, expect, it } from "vitest"

import { buildXtermTheme, xtermMinimumContrastRatio } from "./xtermTheme"
// @ts-expect-error Node types are excluded from the browser tsconfig; Vitest runs this test in Node.
import { readFileSync } from "node:fs"

const styles = readFileSync("src/styles.css", "utf8")

function relativeLuminance(hex: string): number {
    const channels = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    const [r, g, b] = channels.map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrastRatio(a: string, b: string): number {
    const [light, dark] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x)
    return (light + 0.05) / (dark + 0.05)
}

function lightTerminalBackground(): string {
    const lightTokens = styles.slice(styles.indexOf(":root {"), styles.indexOf(".dark {"))
    const match = /--term-bg:\s*(#[0-9a-f]{6})/i.exec(lightTokens)
    if (!match) throw new Error("light --term-bg not found in styles.css")
    return match[1]
}

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
        expect(light.white).toBe("#6e6a61")
        expect(light.brightBlack).toBe("#8a8691")
        expect(light.brightRed).toBe("#d65f5f")
        expect(light.brightGreen).toBe("#42a870")
        expect(light.brightYellow).toBe("#c4841c")
        expect(light.brightBlue).toBe("#3d6df0")
        expect(light.brightMagenta).toBe("#a86bd6")
        expect(light.brightCyan).toBe("#3198a3")
        expect(light.brightWhite).toBe("#2e2b27")

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

    it("keeps light ANSI white text readable on the light terminal background", () => {
        // PowerShell draws ordinary arguments in white (37) and numbers in bright white (97).
        const background = lightTerminalBackground()
        const light = buildXtermTheme("light")

        for (const color of [light.white!, light.brightWhite!]) {
            expect(contrastRatio(color, background)).toBeGreaterThanOrEqual(4.5)
            // Reaching 4.5:1 caps white's distance from black (5.9:1) at about 1.3.
            expect(contrastRatio(color, light.black!)).toBeGreaterThanOrEqual(1.25)
            expect(contrastRatio(color, light.brightBlack!)).toBeGreaterThanOrEqual(1.25)
        }
    })
})

describe("xtermMinimumContrastRatio", () => {
    it("lifts low-contrast foregrounds in light mode only", () => {
        // PSReadLine selects text as black on white (30;47); the darker light white needs the safety net.
        expect(xtermMinimumContrastRatio("light")).toBe(3)
        expect(xtermMinimumContrastRatio("dark")).toBe(1)
    })
})
