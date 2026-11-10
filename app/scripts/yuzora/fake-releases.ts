// yuzora(release): a stand-in for github.com and api.github.com, for checking
// an installed app's update against a release before it is published
// (docs/beta2/release.md, "Before the tag"):
//
//   bun scripts/yuzora/fake-releases.ts <folder> --version 0.1.0 [--port 8765]
//   YUZORA_UPDATE_URL=http://127.0.0.1:8765 <the app>
//
// <folder> holds what the release would: yuzora-update.json, its .sig, the
// archives, the changelogs. It is served as the release list the app asks
// for and as that release's downloads, laid out like GitHub's. Whatever is
// served, the app still believes only a manifest signed by its trusted key.
import { existsSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { REPO, checkVersion } from "./manifest";

export function serve(dir: string, version: string, port = 0, log: (line: string) => void = () => {}) {
  checkVersion(version);
  const tag = `v${version}`;
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port,
    fetch(req) {
      const url = new URL(req.url);
      log(`${req.method} ${url.pathname}${url.search}`);
      const base = `http://127.0.0.1:${server.port}`;
      const downloads = `/${REPO}/releases/download/${tag}/`;
      if (url.pathname === `/repos/${REPO}/releases`) {
        const files = readdirSync(dir).filter((n) => statSync(join(dir, n)).isFile()).sort();
        const assets = files.map((name) => ({ name, size: statSync(join(dir, name)).size, browser_download_url: `${base}${downloads}${name}` }));
        const release = (tag_name: string, assets: unknown[]) => ({ tag_name, draft: false, prerelease: tag_name.includes("-"), assets });
        return Response.json([release(tag, assets), release("v0.0.19", [{ name: "latest.json", browser_download_url: `${base}/${REPO}/releases/download/v0.0.19/latest.json` }])]);
      }
      if (url.pathname.startsWith(downloads)) {
        const name = decodeURIComponent(url.pathname.slice(downloads.length));
        const file = resolve(dir, name);
        if (/^[A-Za-z0-9._-]+$/.test(name) && file === join(resolve(dir), name) && existsSync(file) && statSync(file).isFile()) return new Response(Bun.file(file));
      }
      return new Response("Not Found", { status: 404 });
    },
  });
  return server;
}

if (import.meta.main) {
  const { values, positionals } = parseArgs({ args: process.argv.slice(2), allowPositionals: true, options: { version: { type: "string" }, port: { type: "string", default: "8765" } } });
  if (positionals.length !== 1 || !values.version) {
    console.error("usage: bun scripts/yuzora/fake-releases.ts <folder> --version <x.y.z> [--port 8765]");
    process.exit(2);
  }
  const server = serve(positionals[0], values.version, Number(values.port), (line) => console.log(`${new Date().toISOString()} ${line}`));
  console.log(`serving ${resolve(positionals[0])} as v${values.version}: YUZORA_UPDATE_URL=http://127.0.0.1:${server.port}`);
}
