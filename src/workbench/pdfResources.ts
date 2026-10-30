// pdf.js' built-in binary data (Adobe CMaps for CJK text, standard fonts, and
// the JPX/JBIG2 image decoders), bundled instead of fetched from a CDN. Each
// file is a lazy chunk holding a data URL: loading it is a same-origin dynamic
// import, which `script-src 'self'` already allows on every platform, so no
// fetch of the custom app scheme (or connect-src change) is involved.
import openJpegFallback from "pdfjs-dist/wasm/openjpeg_nowasm_fallback.js?url"

import { decodeBase64 } from "./pdfDocument"

type LazyDataUrl = () => Promise<string>

const cMaps = import.meta.glob<string>("/node_modules/pdfjs-dist/cmaps/*.bcmap", {
    query: "?url&inline",
    import: "default"
})
// Foxit (PDFium, BSD) only: with system fonts on, pdf.js asks for standard
// font data just for Symbol and ZapfDingbats. The Liberation .ttf files are
// GPLv2 and left out; a request for one falls back to a system font.
const standardFonts = import.meta.glob<string>("/node_modules/pdfjs-dist/standard_fonts/*.pfb", {
    query: "?url&inline",
    import: "default"
})
// qcms (ICC colour) only runs with worker-side fetch, which is off here.
const wasm = import.meta.glob<string>(
    ["/node_modules/pdfjs-dist/wasm/openjpeg.wasm", "/node_modules/pdfjs-dist/wasm/jbig2.wasm"],
    { query: "?url&inline", import: "default" }
)

function byFileName(modules: Record<string, LazyDataUrl>): Map<string, LazyDataUrl> {
    return new Map(Object.entries(modules).map(([path, load]) => [path.slice(path.lastIndexOf("/") + 1), load]))
}

/**
 * `wasmUrl` for getDocument. When openjpeg.wasm does not compile (WKWebView's
 * JavaScriptCore keeps relaxed SIMD off), the worker imports
 * `${wasmUrl}openjpeg_nowasm_fallback.js` itself, bypassing the factory, so
 * that file ships as a same-origin asset under its own name (see
 * assetFileNames in vite.config.ts) and loads only on that path.
 */
export const wasmUrl = openJpegFallback.slice(0, openJpegFallback.lastIndexOf("/") + 1)

const tables: Record<string, Map<string, LazyDataUrl>> = {
    cMapUrl: byFileName(cMaps),
    standardFontDataUrl: byFileName(standardFonts),
    wasmUrl: byFileName(wasm)
}

function dataUrlBytes(url: string): Uint8Array {
    const comma = url.indexOf(",")
    if (!url.startsWith("data:") || !url.slice(0, comma).endsWith(";base64")) {
        throw new Error("pdf.js resource is not an inline base64 data URL")
    }
    return decodeBase64(url.slice(comma + 1))
}

/** `BinaryDataFactory` for getDocument (used with `useWorkerFetch: false`). */
export class BundledBinaryDataFactory {
    async fetch({ kind, filename }: { kind: string; filename: string }): Promise<Uint8Array> {
        const load = tables[kind]?.get(filename)
        if (!load) throw new Error(`Unknown pdf.js resource: ${kind} ${filename}`)
        return dataUrlBytes(await load())
    }
}
