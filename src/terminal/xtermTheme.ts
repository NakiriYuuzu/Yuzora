import type { ITheme } from "@xterm/xterm"

type ThemeMode = "light" | "dark"
type Ansi16Theme = Required<
    Pick<
        ITheme,
        | "black"
        | "red"
        | "green"
        | "yellow"
        | "blue"
        | "magenta"
        | "cyan"
        | "white"
        | "brightBlack"
        | "brightRed"
        | "brightGreen"
        | "brightYellow"
        | "brightBlue"
        | "brightMagenta"
        | "brightCyan"
        | "brightWhite"
    >
>

const ansi16Palettes: Record<ThemeMode, Ansi16Theme> = {
    light: {
        black: "#5c5a55",
        red: "#b43d3d",
        green: "#2f8f5f",
        yellow: "#a8690f",
        blue: "#2456cc",
        magenta: "#8a4dbf",
        cyan: "#1f7f8a",
        white: "#f7f3ea",
        brightBlack: "#8a8691",
        brightRed: "#d65f5f",
        brightGreen: "#42a870",
        brightYellow: "#c4841c",
        brightBlue: "#3d6df0",
        brightMagenta: "#a86bd6",
        brightCyan: "#3198a3",
        brightWhite: "#ffffff"
    },
    dark: {
        black: "#0f0e13",
        red: "#ff6b6b",
        green: "#74d6a0",
        yellow: "#e0b06a",
        blue: "#82b4ff",
        magenta: "#c792ea",
        cyan: "#6bd3e6",
        white: "#d6d3db",
        brightBlack: "#847f8b",
        brightRed: "#ff8f8f",
        brightGreen: "#9be8bb",
        brightYellow: "#f0c987",
        brightBlue: "#a8caff",
        brightMagenta: "#d8b0f2",
        brightCyan: "#91e2f0",
        brightWhite: "#ffffff"
    }
}

const fallbacks = {
    "--term-bg": "#18171c",
    "--term-bar": "#201f25",
    "--term-fg": "#d6d3db",
    "--term-fg2": "#847f8b",
    "--term-line": "rgba(255, 255, 255, 0.07)",
    "--term-chip": "rgba(255, 255, 255, 0.1)",
    "--term-hover": "rgba(255, 255, 255, 0.06)",
    "--term-green": "#74d6a0",
    "--term-blue": "#82b4ff",
    "--term-lime": "#b6e36a",
    "--term-coral": "#ff9d6b",
    "--term-ok": "#74d6a0",
    "--term-amber": "#e0b06a"
}

function readTerminalTokens(styles: CSSStyleDeclaration) {
    return {
        bg: readToken(styles, "--term-bg"),
        bar: readToken(styles, "--term-bar"),
        fg: readToken(styles, "--term-fg"),
        fg2: readToken(styles, "--term-fg2"),
        line: readToken(styles, "--term-line"),
        chip: readToken(styles, "--term-chip"),
        hover: readToken(styles, "--term-hover"),
        green: readToken(styles, "--term-green"),
        blue: readToken(styles, "--term-blue"),
        lime: readToken(styles, "--term-lime"),
        coral: readToken(styles, "--term-coral"),
        ok: readToken(styles, "--term-ok"),
        amber: readToken(styles, "--term-amber")
    }
}

function currentMode(): ThemeMode {
    return document.documentElement.classList.contains("dark") ? "dark" : "light"
}

function readToken(styles: CSSStyleDeclaration, name: keyof typeof fallbacks): string {
    const value = styles.getPropertyValue(name).trim()
    return value || fallbacks[name]
}

export function buildXtermTheme(mode: ThemeMode = currentMode()): ITheme {
    const tokens = readTerminalTokens(getComputedStyle(document.documentElement))

    return {
        background: tokens.bg,
        foreground: tokens.fg,
        cursor: tokens.green,
        cursorAccent: tokens.bg,
        selectionBackground: tokens.chip,
        selectionForeground: tokens.fg,
        selectionInactiveBackground: tokens.hover,
        scrollbarSliderBackground: tokens.chip,
        scrollbarSliderHoverBackground: tokens.hover,
        scrollbarSliderActiveBackground: tokens.line,
        overviewRulerBorder: tokens.line,
        ...ansi16Palettes[mode]
    }
}
