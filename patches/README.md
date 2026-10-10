# xterm 6.0.0: Windows font measurement

`@xterm%2Fxterm@6.0.0.patch` recreates the four hidden DOM measurement spans
and their container whenever the font changes. It preserves the visible rows,
terminal buffer, input element and session.

In Windows WebView2 153, reusing these nodes can return the **previous** font's
width during a synchronous layout read. xterm caches that value and compensates
with incorrect letter spacing. At 26px, five characters occupied about 114px
instead of 78px; shrinking to 10px produced approximately -9.59px spacing.
The same production HERDR component and synthetic frames reproduce the problem
without a HERDR server. A direct xterm font change also reproduces it.

Recreating only the measurement subtree makes the first read use the new font.
The patch includes TypeScript source and both published JavaScript entry points;
the large diff is from their minified distribution lines. Bun applies it through
`patchedDependencies`, including `bun install --frozen-lockfile` in CI.

Run `fixtures/xterm-font-regression.html` via Vite, or bundle it as a static page
and open it in Windows WebView2. The visible regression checks actual glyph
widths, bold/italic text, CJK and the bottom marker across repeated font changes.
It must be run on Windows: jsdom cannot verify glyph layout, and the macOS
browser passed even without this patch. Repeat it before removing this patch or
upgrading xterm. See `docs/testing/windows-v0.0.15.md` for packaged-app acceptance.

# xterm 6.0.0: block element glyphs (U+2580-U+259F)

Claude Code draws its mascot with block elements (`▐▛███▜▌`, quadrants, eighths). The
bundled JetBrains Mono subsets exclude U+2580-U+259F, so the browser falls back to CJK
fonts (PingFang, Hiragino...) whose block glyphs do not fill the terminal cell, and
the mascot shows gaps. xterm's `customGlyphs` only exists in the canvas/WebGL renderers;
Yuzora uses the DOM renderer only and WebGL was declined.

The same patch therefore changes `DomRendererRowFactory.createRow`: a cell whose
content is exactly one code point in U+2580-U+259F becomes its own span (never merged
with neighbours or joiners), its text is a single space (so the span is exactly
`width * cellWidth` wide using the space's cached width), and it gets the classes
`xterm-block-glyph xterm-block-<hex>` (for example `xterm-block-259b`). Colours, inverse,
bold/dim, cursor, selection, decorations and minimum contrast (already skipped by xterm
for box/block glyphs) are unchanged. `src/terminal/xtermBlockGlyphs.css` paints each
class with `linear-gradient(currentColor, ...)` background layers (never
`background-color`, which belongs to xterm). It is imported next to every `xterm.css`
import (`main.tsx`, `Demo.tsx`, `HerdrNativeDialog.tsx`, `MachineInteractiveDialog.tsx`).

Files changed by the patch: `src/browser/renderer/dom/DomRendererRowFactory.ts` and both
minified bundles `lib/xterm.js` / `lib/xterm.mjs` (search for `XBg` / `blockCode`).

Tests: `xtermBlockGlyphs.test.ts` (stylesheet covers all 32 code points and uses only
background-image/size/position/repeat) and `src/terminal/xtermBlockGlyphsRenderer.test.ts`
(real `Terminal` in jsdom: block cells are separate spans, normal text still merges).

GUI acceptance: run `bunx vite --config fixtures/e2e.vite.config.ts` and open
`/fixtures/xterm-block-glyphs.html`. Check the glyph grid, the contiguous and tile rows
and the mascot at 12/13/16/20px, every font preset and the light/dark toggle: adjacent
cells must tile with no seams or gaps.

When upgrading xterm: regenerate with `bun patch @xterm/xterm@<version>`, re-apply both
the Windows measurement fix and this change in the TypeScript source and in both
bundles, run `bun patch --commit node_modules/@xterm/xterm` (delete the `.bun-tag-*`
hunk it adds to the patch file), then remove `node_modules/@xterm/xterm`, run
`bun install --frozen-lockfile`, grep the installed bundles for `XBg`, and rerun the two
tests and the fixture above.
