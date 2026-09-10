# Yuzora product media

Remotion sources for Yuzora GitHub Pages and README media. Registered compositions use recordings of the current AppShell demo, including the real editor, Git diff, database and settings UI. The demo uses sample data; see [RECORDING.md](RECORDING.md) to refresh captures.

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
bunx remotion still ade-herdr-en /tmp/ade-herdr-en.png --frame=148 --scale=1
```

All animation timing must come from Remotion frames (`useCurrentFrame`, `interpolate`, or `Sequence`), not CSS animations or transitions.

## README images and tour

After refreshing the captures, run these commands from the repository root:

```bash
bun scripts/render-site-media.ts
bun scripts/render-readme-media.ts
```

The second command renders six PNGs to `docs/readme/`, copies the same Remotion frames to the Pages posters, and renders `hero-en.gif` / `hero-zh.gif` from `readme-tour-en` / `readme-tour-zh`. Each 21-second tour shows seven seconds each of the current terminal, Git and database recordings, at 880×587 and 6 fps. The legacy hand-drawn Hero composition is not registered or used.

Inspect the rendered stills and all three tour sections before committing. The database still should show SQL results, not the later appearance dialog; the Git still should show the current history button and diff.
