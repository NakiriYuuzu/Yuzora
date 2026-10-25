import { describe, expect, it } from "vitest"

// Every shipped (non-test) source file of the phone app, as text.
const sources = import.meta.glob<string>(["/src/companion/**/*.{ts,tsx}", "!/src/companion/**/*.test.{ts,tsx}"], {
  query: "?raw",
  import: "default",
  eager: true,
})
const files = Object.entries(sources)

const CLIPBOARD = [
  /navigator\s*\??\.\s*clipboard/i,
  /clipboard/i, // ClipboardAddon, ClipboardItem, clipboardData, clipboard-write
  /execCommand/,
  /\bon(copy|cut|paste)\b/i,
  /["'`](copy|cut|paste)["'`]/,
]
const LINKS = [/<a[\s>]/, /\bhref\s*=/, /dangerouslySetInnerHTML/, /\binnerHTML\b/, /\bmarked\b|markdown|remark|rehype|linkify|WebLinks/i, /location\s*\.\s*(href|assign|replace)\s*[=(]/, /\.open\(\s*[^)]/]

function hits(patterns: RegExp[], allowed: (file: string) => boolean = () => false) {
  return files.flatMap(([file, text]) =>
    allowed(file) ? [] : patterns.filter((pattern) => pattern.test(text)).map((pattern) => `${file}: ${pattern}`))
}

describe("phone app threat rows", () => {
  it("sees every source file (the scan cannot pass by scanning nothing)", () => {
    expect(files.length).toBeGreaterThan(40)
    for (const known of ["/src/companion/components/useTerminal.ts", "/src/companion/lib/rpc.ts", "/src/companion/screens/ControlPanel.tsx", "/src/companion/client.ts"]) {
      expect(Object.keys(sources), known).toContain(known)
    }
    expect(files.some(([file]) => file.endsWith(".test.ts") || file.endsWith(".test.tsx"))).toBe(false)
  })

  it("never touches the clipboard: no Clipboard API, copy/paste events, execCommand or xterm clipboard addon", () => {
    expect(hits(CLIPBOARD)).toEqual([])
  })

  it("renders no clickable links of its own: no anchors, hrefs, HTML injection or markdown", () => {
    // Terminal OSC 8 links are the only link surface (see useTerminal.test.tsx);
    // window.open exists in that one file and nowhere else.
    expect(hits(LINKS, (file) => file === "/src/companion/components/useTerminal.ts")).toEqual([])
    const opener = files.filter(([, text]) => /window\s*\.\s*open\s*\(/.test(text)).map(([file]) => file)
    expect(opener).toEqual(["/src/companion/components/useTerminal.ts"])
    const terminal = sources["/src/companion/components/useTerminal.ts"]
    // The window only opens behind a confirmation on the same condition.
    expect(terminal).toMatch(/window\.confirm\([^)]*\)[^\n]*\)\s*\{\s*window\.open\(/)
  })

  it("loads no xterm addon", () => {
    expect(hits([/@xterm\/addon-/, /loadAddon/])).toEqual([])
  })
})
