# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["fonttools[woff]>=4.59,<5", "uharfbuzz>=0.51,<1"]
# ///
"""Rebuild SVGs; add --native to also regenerate Tauri icons and PNG fallbacks."""

import argparse
import json
from io import BytesIO
from pathlib import Path
import re
from shutil import copyfile
import subprocess

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "site/assets/brand"
FONT_DIR = ROOT / "node_modules/@fontsource-variable/hanken-grotesk"

# One 256-unit master supplies the standalone, lockup and construction views.
# The two upper planes leave a 12-unit seam at x=124..136.
MARK = '''<g id="yuzora-mark">
  <path fill="var(--logo-silver, #cbd5e1)" d="M34 32H73Q77 32 80 35L120 77Q124 81 124 86V109Q124 117 118 111L91 83H78Q73 83 70 79L30 39Q24 32 34 32Z"/>
  <path fill="var(--logo-fold, #8caab7)" d="M91 83H124V109Q124 117 118 111Z"/>
  <path fill="var(--logo-primary, #3b82f6)" d="M142 88L190 36Q194 32 199 32H231Q239 32 234 39L149 132Q142 139 136 132V101Q136 94 142 88Z"/>
  <path fill="var(--logo-secondary, #14b8a6)" d="M96 111L132 148Q136 152 136 158V224Q136 232 130 226L92 186Q88 182 88 176V115Q88 107 96 111Z"/>
</g>'''

def document(title: str, viewbox: str, body: str) -> str:
    _, _, width, height = viewbox.split()
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="{viewbox}" role="img" aria-labelledby="title">
<title id="title">{title}</title>
{body}
</svg>
'''


def wordmark() -> tuple[str, float]:
    font = instantiateVariableFont(
        TTFont(FONT_DIR / "files/hanken-grotesk-latin-wght-normal.woff2"),
        {"wght": 600},
        inplace=False,
    )
    font.flavor = None
    data = BytesIO()
    font.save(data)
    shaped_font = hb.Font(hb.Face(data.getvalue()))
    upem = font["head"].unitsPerEm
    shaped_font.scale = (upem, upem)
    buffer = hb.Buffer()
    buffer.add_str("Yuzora")
    buffer.guess_segment_properties()
    hb.shape(shaped_font, buffer, {"kern": True})
    glyphs = font.getGlyphSet()
    order = font.getGlyphOrder()
    scale, x, baseline = 132 / upem, 278.0, 176
    paths = []
    for info, pos in zip(buffer.glyph_infos, buffer.glyph_positions):
        pen = SVGPathPen(glyphs, ntos=lambda v: f"{v:.3f}".rstrip("0").rstrip("."))
        transform = (scale, 0, 0, -scale, x + pos.x_offset * scale, baseline - pos.y_offset * scale)
        glyphs[order[info.codepoint]].draw(TransformPen(pen, transform))
        paths.append(f'  <path d="{pen.getCommands()}"/>')
        x += pos.x_advance * scale
    return '<g fill="var(--logo-ink, #0e1026)">\n' + '\n'.join(paths) + '\n</g>', x


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native", action="store_true", help="Generate platform icons with the Tauri CLI")
    args = parser.parse_args()
    DEST.mkdir(parents=True, exist_ok=True)
    text, end = wordmark()
    width = round(end + 28)
    (DEST / "yuzora-mark.svg").write_text(document("Yuzora symbol", "0 0 256 256", MARK))
    mark_body = MARK.replace(' id="yuzora-mark"', '')
    lockup = f'<g id="yuzora-lockup">\n{mark_body}\n{text}\n</g>'
    (DEST / "yuzora-lockup.svg").write_text(document("Yuzora", f"0 0 {width} 256", lockup))
    grid = ''.join(f'<path d="M{n} 0V256M0 {n}H256"/>' for n in range(0, 257, 16))
    geometry = f'''<g id="yuzora-geometry">
  <g fill="none" stroke="var(--logo-guide, #94a3b8)" stroke-width=".55" opacity=".32">{grid}</g>
  <g opacity=".2">{MARK.replace(' id="yuzora-mark"', '')}</g>
  <g fill="none" stroke="var(--logo-guide, #64748b)" stroke-width=".8">
    <path d="M0 32H256M0 228H256M128 -16V272" stroke-dasharray="3 4"/>
    <path d="M30 39L218 227M231 32L88 188" opacity=".55"/>
    <path d="M124 68V132M136 68V132M124 100H136"/>
    <path d="M0 -10H256M0 -14V-6M256 -14V-6"/>
    <circle cx="34" cy="38" r="6"/><circle cx="130" cy="224" r="6"/>
  </g>
  <g fill="var(--logo-ink, #0e1026)" font-family="monospace" font-size="8">
    <text x="128" y="-17" text-anchor="middle">256 u</text>
    <text x="150" y="103">12 u</text>
    <text x="0" y="273">16 u GRID</text>
  </g>
</g>'''
    (DEST / "yuzora-geometry.svg").write_text(document("Yuzora construction grid", "-24 -32 304 320", geometry))
    copyfile(FONT_DIR / "LICENSE", DEST / "Hanken-Grotesk-OFL.txt")
    app_dest = ROOT / "public/brand"
    app_dest.mkdir(parents=True, exist_ok=True)
    for name in ("yuzora-mark.svg", "yuzora-lockup.svg"):
        copyfile(DEST / name, app_dest / name)

    # Native icon renderers do not inherit CSS variables from the application.
    native_mark = re.sub(r"var\(--[\w-]+,\s*(#[0-9a-f]+)\)", r"\1", mark_body)
    native_body = f'''<rect x="64" y="64" width="896" height="896" rx="196" fill="#0e1026"/>
<g transform="translate(116 116) scale(3)">{native_mark}</g>'''
    native_source = DEST / "yuzora-app-icon.svg"
    native_source.write_text(document("Yuzora app icon", "0 0 1024 1024", native_body))
    # Inline paths avoid external <use> repaint differences in native WebViews.
    paths = [dict(fill=fill, d=d) for fill, d in re.findall(r'<path fill="([^"]+)" d="([^"]+)"', mark_body)]
    (ROOT / "src/components/brand-paths.ts").write_text(
        "// Generated by scripts/generate-brand-assets.py; edit the master there.\n"
        + "export const BRAND_PATHS = " + json.dumps(paths, indent=2) + " as const\n"
    )
    symbols = f'<svg xmlns="http://www.w3.org/2000/svg" style="display:none" aria-hidden="true"><defs>{MARK}{lockup}{geometry}</defs></svg>'
    site_index = ROOT / "site/index.html"
    html = site_index.read_text()
    block = f'<!-- brand-symbols:start -->\n{symbols}\n<!-- brand-symbols:end -->'
    if '<!-- brand-symbols:start -->' in html:
        html = re.sub(r'<!-- brand-symbols:start -->.*?<!-- brand-symbols:end -->', lambda _: block, html, flags=re.S)
    else:
        html = re.sub(r'(<body[^>]*>)', lambda match: match[0] + '\n' + block, html, count=1)
    html = re.sub(r'href="assets/brand/yuzora-(mark|lockup|geometry)\.svg#', 'href="#', html)
    site_index.write_text(html)
    if args.native:
        icon_dir = ROOT / "src-tauri/icons"
        subprocess.run(
            ["bun", "run", "tauri", "icon", str(native_source), "--output", str(icon_dir), "--ios-color", "#0e1026"],
            cwd=ROOT,
            check=True,
        )
        copyfile(icon_dir / "icon.png", ROOT / "public/yuzora.png")
        copyfile(icon_dir / "128x128@2x.png", ROOT / "site/assets/yuzora-icon.png")
    print(f"Generated symbol (256 × 256), lockup ({width} × 256) and construction SVGs in {DEST}")


if __name__ == "__main__":
    main()
