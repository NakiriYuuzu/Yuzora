# Yuzora product media

Remotion sources for the Yuzora GitHub Pages feature videos and README media. The compositions mirror the current ADE + HERDR UI using the same design tokens as the desktop app.

## Commands

```bash
bun install
bun run dev    # Remotion Studio, no automatic browser open
bun run build  # create a deployable Remotion bundle
bun run lint
```

Render the public feature videos:

```bash
for c in ade-herdr-zh ade-herdr-en remote-db-zh remote-db-en terminal-git-zh terminal-git-en; do
  bunx remotion render "$c" "../site/assets/$c.mp4" --scale=2
done
```

Render a still for visual QA:

```bash
bunx remotion still ade-herdr-en /tmp/ade-herdr-en.png --frame=210 --scale=1
```

All animation timing must come from Remotion frames (`useCurrentFrame`, `interpolate`, or `Sequence`), not CSS animations or transitions.
