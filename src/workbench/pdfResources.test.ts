import { describe, expect, it, vi } from "vitest"

import { BundledBinaryDataFactory, wasmUrl } from "./pdfResources"

// Objects are latin1 strings so binary image streams keep their bytes.
function pdfFile(objects: string[]): Uint8Array {
    let pdf = "%PDF-1.7\n"
    const offsets = objects.map((body, i) => {
        const offset = pdf.length
        pdf += `${i + 1} 0 obj\n${body}\nendobj\n`
        return offset
    })
    const xref = pdf.length
    pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`
    pdf += offsets.map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`).join("")
    pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
    return Uint8Array.from(pdf, (char) => char.charCodeAt(0))
}

// One page showing 中文 in a non-embedded CNS1 font: text extraction needs
// the predefined UniCNS-UCS2-H CMap, which pdf.js must request from the factory.
function cjkPdf(): Uint8Array {
    const content = "BT /F1 24 Tf 10 50 Td <4E2D6587> Tj ET"
    return pdfFile([
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 4 0 R >> >> /Contents 7 0 R >>",
        "<< /Type /Font /Subtype /Type0 /BaseFont /MingLiU /Encoding /UniCNS-UCS2-H /DescendantFonts [5 0 R] >>",
        "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /MingLiU /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 0 >> /FontDescriptor 6 0 R >>",
        "<< /Type /FontDescriptor /FontName /MingLiU /Flags 4 /FontBBox [0 -120 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>",
        `<< /Length ${content.length} >>\nstream\n${content}\nendstream`
    ])
}

// An 8×8 JPEG 2000 codestream (sips' JP2 output, jp2c box contents).
const JPX_CODESTREAM = "/0//UQAvAAAAAAAIAAAACAAAAAAAAAAAAAAACAAAAAgAAAAAAAAAAAADBwEBBwEBBwEB/1IADAAAAAEBBQQEAAD/XAAjIncedup26na8bwBvAG7iZ0xnTGdkUANQA1BFV9JX0ldh/2QAEQABS2FrYWR1LXY1LjIuMf+QAAoAAAAAANoAAf+Tw+8CCLHB8wICuMD7AIAFz4CAgICAgMPvAofSCgHAgAiGCYgCwD4wMA4EAALAPgBgPmBQA4EGBrgCz7wafeDR9gKAC6Uq+BezC0ts8vcMD7vsasfA+MJB8wQADgIH8AwKHNfA+MHA+MIADPxDCzULsM+8Tn3icfYIgAygHPVAo99trYIBxMhd7sNcAvYXMuNIjqRUTfryTHAcPNbBHMeJJLDBr1OrEw6zvAEmtBvaRb7AOEgHRAAhZFOIAWEKW8A6JAOiACDmWpQDb0SO/9k="

// One page painting that image through /JPXDecode.
function jpxPdf(): Uint8Array {
    const image = atob(JPX_CODESTREAM)
    const content = "q 80 0 0 80 10 10 cm /Im1 Do Q"
    return pdfFile([
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Im1 4 0 R >> >> /Contents 5 0 R >>",
        `<< /Type /XObject /Subtype /Image /Width 8 /Height 8 /Filter /JPXDecode /Length ${image.length} >>\nstream\n${image}\nendstream`,
        `<< /Length ${content.length} >>\nstream\n${content}\nendstream`
    ])
}

describe("pdf.js with the bundled factory", () => {
    it("loads the CMap it needs through BundledBinaryDataFactory", async () => {
        const requests: string[] = []
        class RecordingFactory extends BundledBinaryDataFactory {
            fetch(request: { kind: string; filename: string }) {
                requests.push(`${request.kind}:${request.filename}`)
                return super.fetch(request)
            }
        }
        const { getDocument, VerbosityLevel } = await import("pdfjs-dist/legacy/build/pdf.mjs")
        const log = vi.spyOn(console, "log").mockImplementation(() => {})
        const task = getDocument({
            data: cjkPdf(),
            BinaryDataFactory: RecordingFactory,
            useWorkerFetch: false,
            verbosity: VerbosityLevel.ERRORS
        })
        try {
            const doc = await task.promise
            const page = await doc.getPage(1)
            const text = await page.getTextContent()
            expect(requests).toContain("cMapUrl:UniCNS-UCS2-H.bcmap")
            expect(text.items.map((item) => ("str" in item ? item.str : "")).join("")).toBe("中文")
        } finally {
            await task.destroy()
            log.mockRestore()
        }
    })
})

// WKWebView's JavaScriptCore rejects openjpeg.wasm (relaxed SIMD is off), so
// pdf.js imports `${wasmUrl}openjpeg_nowasm_fallback.js` instead (#178);
// useWasm: false takes that path directly.
describe("pdf.js JPX decoding without wasm", () => {
    // The paint op stays even when decoding fails; the image object is null then.
    async function decodedImageSizes(fallbackUrl: string | undefined) {
        const { getDocument, OPS, VerbosityLevel } = await import("pdfjs-dist/legacy/build/pdf.mjs")
        const log = vi.spyOn(console, "log").mockImplementation(() => {})
        const task = getDocument({
            data: jpxPdf(),
            BinaryDataFactory: BundledBinaryDataFactory,
            useWorkerFetch: false,
            useWasm: false,
            wasmUrl: fallbackUrl,
            verbosity: VerbosityLevel.ERRORS
        })
        try {
            const page = await (await task.promise).getPage(1)
            const { fnArray, argsArray } = await page.getOperatorList()
            const images = fnArray.flatMap((fn, i) => (fn === OPS.paintImageXObject ? [argsArray[i][0] as string] : []))
                .map((id) => new Promise<{ width: number; height: number } | null>((resolve) => page.objs.get(id, resolve)))
            return (await Promise.all(images)).map((image) => image && [image.width, image.height])
        } finally {
            await task.destroy()
            log.mockRestore()
        }
    }

    it("decodes the image with the JS fallback served from wasmUrl", async () => {
        expect(wasmUrl).toMatch(/^\/.*\/$/)
        // The app serves wasmUrl from the Vite root, two levels up from here.
        // (Not `new URL(…, import.meta.url)` inline: Vite rewrites that pattern.)
        const here = import.meta.url
        expect(await decodedImageSizes(new URL(`../..${wasmUrl}`, here).href)).toEqual([[8, 8]])
    })

    it("loses the image when the fallback cannot be imported", async () => {
        expect(await decodedImageSizes(undefined)).toEqual([null])
    })
})

// Exercises the real lazy glob imports, so a renamed pdfjs-dist resource
// directory or a Vite query change fails here instead of in the app.
describe("BundledBinaryDataFactory", () => {
    const factory = new BundledBinaryDataFactory()

    it("serves the packed CJK CMaps pdf.js asks for", async () => {
        const bytes = await factory.fetch({ kind: "cMapUrl", filename: "UniCNS-UCS2-H.bcmap" })
        expect(bytes.byteLength).toBeGreaterThan(1000)
    })

    it("serves the standard font files", async () => {
        const bytes = await factory.fetch({ kind: "standardFontDataUrl", filename: "FoxitSymbol.pfb" })
        expect(bytes.byteLength).toBeGreaterThan(1000)
    })

    it("serves the image decoder wasm", async () => {
        const bytes = await factory.fetch({ kind: "wasmUrl", filename: "openjpeg.wasm" })
        expect([...bytes.subarray(0, 4)]).toEqual([0x00, 0x61, 0x73, 0x6d])
    })

    it("rejects files it does not bundle", async () => {
        await expect(factory.fetch({ kind: "cMapUrl", filename: "../../package.json" })).rejects.toThrow(/Unknown pdf\.js resource/)
        await expect(factory.fetch({ kind: "wasmUrl", filename: "quickjs-eval.wasm" })).rejects.toThrow(/Unknown pdf\.js resource/)
        await expect(factory.fetch({ kind: "standardFontDataUrl", filename: "LiberationSans-Regular.ttf" })).rejects.toThrow(/Unknown pdf\.js resource/)
        await expect(factory.fetch({ kind: "iccUrl", filename: "CGATS001Compat-v2-micro.icc" })).rejects.toThrow(/Unknown pdf\.js resource/)
    })
})
