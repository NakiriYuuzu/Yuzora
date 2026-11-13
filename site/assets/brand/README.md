# Yuzora brand assets

- `yuzora-mark.svg`: the standalone symbol on a transparent canvas.
- `yuzora-lockup.svg`: the symbol and outlined Yuzora wordmark on a transparent canvas.
- `yuzora-geometry.svg`: construction grid and measurements from the same master.
- `yuzora-app-icon.svg`: the fixed-palette, padded midnight tile the Tauri app's platform icons were generated from.

The mark is a vector reconstruction of the selected reference: silver upper-left plane, cobalt upper-right plane, sea-glass stem and a small folded edge. It uses a 256-unit canvas, a 16-unit reference grid and a 12-unit central seam. The typography uses Hanken Grotesk at weight 600, shaped with HarfBuzz and converted to paths. The exported logos need no fonts, scripts, raster images or remote resources. The font's SIL Open Font License is included separately.

## Theme integration

The SVG shapes accept `--logo-primary`, `--logo-secondary`, `--logo-silver`, `--logo-fold` and `--logo-ink`. The construction drawing also accepts `--logo-guide`. Standalone files use the reference palette with dark lettering as a stable default for graphics applications; the website supplies explicit colors for both light and dark appearances.

The website embeds generated, same-document SVG groups with local `<use>` references, so its CSS variables inherit into the artwork. An SVG loaded through `<img>` does not inherit the enclosing page's variables. For another site or application, use inline SVG or a same-origin `<use>` instance and map these variables to that application's theme tokens. For a fixed-color export, replace the `var(...)` fills with the desired colors.

```html
<svg viewBox="0 0 256 256" role="img" aria-label="Yuzora">
  <use href="yuzora-mark.svg#yuzora-mark" />
</svg>
```

The SVGs were generated from the shared master by `scripts/generate-brand-assets.py`, which left the repository with the Tauri app (last at tag `v0.1.2`). They are kept here as committed files.

The website is published from `site/` by `.github/workflows/site.yml` when a change under `site/` is pushed to `main`.
