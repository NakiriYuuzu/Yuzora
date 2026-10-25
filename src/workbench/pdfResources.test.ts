import { describe, expect, it, vi } from "vitest"

import { BundledBinaryDataFactory } from "./pdfResources"

// One page showing 中文 in a non-embedded CNS1 font: text extraction needs
// the predefined UniCNS-UCS2-H CMap, which pdf.js must request from the factory.
function cjkPdf(): Uint8Array {
    const content = "BT /F1 24 Tf 10 50 Td <4E2D6587> Tj ET"
    const objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 4 0 R >> >> /Contents 7 0 R >>",
        "<< /Type /Font /Subtype /Type0 /BaseFont /MingLiU /Encoding /UniCNS-UCS2-H /DescendantFonts [5 0 R] >>",
        "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /MingLiU /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 0 >> /FontDescriptor 6 0 R >>",
        "<< /Type /FontDescriptor /FontName /MingLiU /Flags 4 /FontBBox [0 -120 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>",
        `<< /Length ${content.length} >>\nstream\n${content}\nendstream`
    ]
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
    return new TextEncoder().encode(pdf)
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
