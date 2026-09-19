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
