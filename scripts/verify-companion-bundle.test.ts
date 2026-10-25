import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { afterEach, describe, expect, it } from "vitest"
import { validStaticSegment, verifyCompanionBundle } from "./verify-companion-bundle"

const SHELL: Record<string, string> = {
  "index.html": `<!doctype html><html><head><link rel="manifest" href="/manifest.webmanifest" /><link rel="apple-touch-icon" href="/icons/a.png" /><script type="module" crossorigin src="/assets/index-a1.js"></script><link rel="stylesheet" href="/assets/index-b2.css"></head><body><div id="root"></div></body></html>`,
  "assets/index-a1.js": "console.log(1)",
  "assets/index-b2.css": "@font-face{src:url(/assets/font-c3.woff2)}",
  "assets/font-c3.woff2": "font",
  "icons/a.png": "png",
  "icons/b.png": "png",
  "manifest.webmanifest": JSON.stringify({ start_url: "/", scope: "/", display: "standalone", icons: [{ src: "/icons/a.png", sizes: "192x192" }, { src: "/icons/b.png", sizes: "512x512" }] }),
  "sw.js": `const PRECACHE = ${JSON.stringify(["/", "/assets/font-c3.woff2", "/assets/index-a1.js", "/assets/index-b2.css", "/icons/a.png", "/icons/b.png", "/manifest.webmanifest"])}\n`,
}

let dir = ""
afterEach(async () => {
  if (dir) await rm(dir, { recursive: true, force: true })
  dir = ""
})

async function bundle(changes: Record<string, string | null> = {}): Promise<string> {
  dir = await mkdtemp(join(tmpdir(), "yzc-bundle-"))
  for (const [name, content] of Object.entries({ ...SHELL, ...changes })) {
    if (content === null) continue
    await mkdir(dirname(join(dir, name)), { recursive: true })
    await writeFile(join(dir, name), content)
  }
  return dir
}

describe("verifyCompanionBundle", () => {
  it("accepts a shell the gateway serves completely", async () => {
    const result = await verifyCompanionBundle(await bundle())
    expect(result.files).toEqual(Object.keys(SHELL).sort())
    expect(result.gzipBytes).toBeGreaterThan(0)
  })

  it.each([
    ["an inline script", { "index.html": SHELL["index.html"].replace("</body>", "<script>alert(1)</script></body>") }, "inline script"],
    ["an inline handler", { "index.html": SHELL["index.html"].replace("<body>", `<body onload="x()">`) }, "inline event handler"],
    ["a script from another origin", { "index.html": SHELL["index.html"].replace("/assets/index-a1.js", "https://cdn.example/x.js") }, "is not a local path"],
    ["a protocol-relative stylesheet", { "index.html": SHELL["index.html"].replace("/assets/index-b2.css", "//cdn.example/x.css") }, "is not a local path"],
    ["a data: font", { "assets/index-b2.css": "@font-face{src:url(data:font/woff2;base64,AA)}" }, "is not a local path"],
    ["a runtime CSS import", { "assets/index-b2.css": "@import url(/assets/x.css);" }, "runtime @import"],
    ["a desktop API", { "assets/index-a1.js": "window.__TAURI_INTERNALS__" }, "desktop-only __TAURI"],
    ["a source map", { "assets/index-a1.js.map": "{}" }, "not allowed in the shell"],
    ["a file outside the routes", { "robots.txt": "x" }, "outside the served routes"],
    ["a name the gateway refuses", { "assets/con.js": "x" }, "not a servable path"],
    ["a missing service worker", { "sw.js": null }, "sw.js: missing"],
    ["an unprecached asset", { "assets/extra-d4.js": "x" }, "does not precache /assets/extra-d4.js"],
    ["a precached file that is absent", { "sw.js": SHELL["sw.js"].replace("\"/icons/b.png\"", "\"/icons/b.png\",\"/icons/gone.png\"") }, "precaches /icons/gone.png"],
    ["an unfilled placeholder", { "sw.js": `${SHELL["sw.js"]}const BUILD = "__YZC_BUILD__"\n` }, "unfilled placeholder"],
    ["a manifest without a 512px icon", { "manifest.webmanifest": JSON.stringify({ start_url: "/", scope: "/", display: "standalone", icons: [{ src: "/icons/a.png", sizes: "192x192" }] }) }, "no 512x512 icon"],
    ["a manifest that starts elsewhere", { "manifest.webmanifest": SHELL["manifest.webmanifest"].replace("\"start_url\":\"/\"", "\"start_url\":\"/pair\"") }, "start_url/scope"],
  ])("rejects %s", async (_, changes, message) => {
    await expect(verifyCompanionBundle(await bundle(changes))).rejects.toThrow(message)
  })
})

describe("validStaticSegment", () => {
  it("mirrors the gateway's static path rule", () => {
    for (const name of ["index-a1.js", "_x", "-x", "a.b.c", "COM10", "console.js"]) expect(validStaticSegment(name), name).toBe(true)
    for (const name of ["", ".hidden", "a.", "a b", "a%2e", "CON", "nul.txt", "Com1.js", "lpt9", "é.js"]) expect(validStaticSegment(name), name).toBe(false)
  })
})
