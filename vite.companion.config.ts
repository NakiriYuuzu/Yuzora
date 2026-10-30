import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";

const repo = import.meta.dirname;
/** Staging output (`COMPANION_WEB_STAGING` in scripts/verify-companion-bundle.ts): verified, then synced into the web root. */
const STAGING = path.resolve(repo, "src-tauri/target/companion/web");
const BRAND = "#0e1026";

// The gateway serves `/icons/*` verbatim; reuse the app's own icon files.
const ICONS: [string, string][] = [
  ["icon-192.png", "src-tauri/icons/android/mipmap-xxxhdpi/ic_launcher.png"],
  ["icon-512.png", "src-tauri/icons/icon.png"],
  ["apple-touch-icon.png", "src-tauri/icons/ios/AppIcon-60x60@3x.png"],
  ["favicon-32.png", "src-tauri/icons/32x32.png"],
];

const MANIFEST = {
  id: "/",
  name: "Yuzora",
  short_name: "Yuzora",
  start_url: "/",
  scope: "/",
  display: "standalone",
  background_color: BRAND,
  theme_color: BRAND,
  icons: [
    { src: "/icons/icon-192.png", sizes: "192x192", type: "image/png" },
    { src: "/icons/icon-512.png", sizes: "512x512", type: "image/png" },
  ],
};

/**
 * Emits the icons, the manifest and `/sw.js`. The service worker's precache
 * list and build id derive from the bundle's contents only: the same sources
 * always produce the same files, so an unchanged build rewrites nothing.
 */
function companionShell(): Plugin {
  return {
    name: "yuzora-companion-shell",
    apply: "build",
    enforce: "post",
    generateBundle(_, bundle) {
      const files = new Map<string, string | Uint8Array>();
      for (const [name, item] of Object.entries(bundle)) files.set(name, item.type === "chunk" ? item.code : item.source);
      for (const [name, source] of ICONS) files.set(`icons/${name}`, readFileSync(path.resolve(repo, source)));
      files.set("manifest.webmanifest", `${JSON.stringify(MANIFEST, null, 2)}\n`);
      const hash = createHash("sha256");
      const names = [...files.keys()].sort();
      for (const name of names) {
        const source = files.get(name)!;
        if (!(name in bundle)) this.emitFile({ type: "asset", fileName: name, source });
        hash.update(name).update("\0").update(source).update("\0");
      }
      const precache = ["/", ...names.filter((name) => name !== "index.html").map((name) => `/${name}`)];
      const template = readFileSync(path.resolve(repo, "companion/sw.js"), "utf8");
      const source = template
        .replace("\"__YZC_BUILD__\"", JSON.stringify(hash.digest("hex").slice(0, 16)))
        .replace("[\"__YZC_PRECACHE__\"]", JSON.stringify(precache));
      if (source.includes("__YZC_")) throw new Error("companion/sw.js placeholders not replaced");
      this.emitFile({ type: "asset", fileName: "sw.js", source });
    },
  };
}

export default defineConfig({
  root: path.resolve(repo, "companion"),
  base: "/",
  publicDir: false,
  plugins: [react(), tailwindcss(), companionShell()],
  resolve: { alias: { "@yuzora/shared": path.resolve(repo, "packages/shared/src") } },
  build: {
    outDir: STAGING,
    emptyOutDir: true,
    assetsDir: "assets",
    // The gateway CSP has no data: fonts; keep every asset a file.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    reportCompressedSize: false,
    // One chunk on purpose: fewer requests against the static rate limit.
    chunkSizeWarningLimit: 1024,
  },
  server: { host: "127.0.0.1", port: 4175, strictPort: true },
});
