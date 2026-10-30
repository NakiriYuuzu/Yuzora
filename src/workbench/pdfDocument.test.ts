import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { remoteFilePath } from "@/lib/runtimeIdentity"

const ipc = vi.hoisted(() => ({
    readFileBase64: vi.fn(),
    openFile: vi.fn()
}))
vi.mock("@/lib/ipc", () => ipc)

const pdfjs = vi.hoisted(() => ({
    getDocument: vi.fn(),
    PDFWorker: { create: vi.fn() }
}))
vi.mock("pdfjs-dist/legacy/build/pdf.mjs", () => pdfjs)

import {
    LOCAL_PDF_MAX_BYTES,
    REMOTE_PDF_MAX_BYTES,
    decodeBase64,
    isPasswordError,
    openPdfDocument,
    readPdfBytes
} from "./pdfDocument"
import { BundledBinaryDataFactory, wasmUrl } from "./pdfResources"

const remotePdf = remoteFilePath("devbox", "/home/me/docs/spec.pdf", "/home/me/docs")

describe("readPdfBytes", () => {
    beforeEach(() => {
        ipc.readFileBase64.mockReset()
        ipc.openFile.mockReset()
    })

    it("decodes the base64 payload under the local limit", async () => {
        ipc.readFileBase64.mockResolvedValue({ data: btoa("%PDF-1.7"), size: 8 })
        const result = await readPdfBytes("/w/spec.pdf")
        expect(ipc.readFileBase64).toHaveBeenCalledWith("/w/spec.pdf", LOCAL_PDF_MAX_BYTES)
        expect(result.kind === "ok" && [result.size, String.fromCharCode(...result.bytes)]).toEqual([8, "%PDF-1.7"])
    })

    it("uses the host protocol limit for remote workspaces", async () => {
        ipc.readFileBase64.mockResolvedValue({ data: "", size: 0 })
        await readPdfBytes(remotePdf)
        expect(ipc.readFileBase64).toHaveBeenCalledWith(remotePdf, REMOTE_PDF_MAX_BYTES)
    })

    it("reports a local file over the limit with its size", async () => {
        ipc.readFileBase64.mockRejectedValue("file too large: 73400320 bytes (max 52428800)")
        ipc.openFile.mockResolvedValue({ kind: "tooLarge", size: 73400320 })
        await expect(readPdfBytes("/w/huge.pdf")).resolves.toEqual({
            kind: "tooLarge",
            size: 73400320,
            limit: LOCAL_PDF_MAX_BYTES
        })
    })

    it("reports a remote file over the limit even when its size is unknown", async () => {
        ipc.readFileBase64.mockRejectedValue(new Error("file-too-large"))
        ipc.openFile.mockRejectedValue("file-too-large")
        await expect(readPdfBytes(remotePdf)).resolves.toEqual({
            kind: "tooLarge",
            size: null,
            limit: REMOTE_PDF_MAX_BYTES
        })
    })

    it("rethrows other read failures", async () => {
        ipc.readFileBase64.mockRejectedValue("stat failed: No such file or directory")
        await expect(readPdfBytes("/w/gone.pdf")).rejects.toBe("stat failed: No such file or directory")
        expect(ipc.openFile).not.toHaveBeenCalled()
    })
})

describe("openPdfDocument", () => {
    const workers: Array<{ url: string; options: WorkerOptions; terminate: ReturnType<typeof vi.fn>; target: EventTarget }> = []
    const pdfWorker = { destroy: vi.fn() }

    beforeEach(() => {
        workers.length = 0
        pdfWorker.destroy.mockReset()
        pdfjs.getDocument.mockReset()
        pdfjs.PDFWorker.create.mockReset().mockReturnValue(pdfWorker)
        vi.stubGlobal("Worker", class extends EventTarget {
            terminate = vi.fn()
            constructor(url: string, options: WorkerOptions) {
                super()
                workers.push({ url, options, terminate: this.terminate, target: this })
            }
        })
    })

    afterEach(() => {
        vi.unstubAllGlobals()
    })

    function loadingTask(promise: Promise<unknown>) {
        return { promise, destroy: vi.fn(() => Promise.resolve()) }
    }

    it("parses on a same-origin module worker with bundled binary data", async () => {
        const doc = { numPages: 2 }
        const task = loadingTask(Promise.resolve(doc))
        pdfjs.getDocument.mockReturnValue(task)
        const bytes = new Uint8Array([1, 2, 3])

        const handle = await openPdfDocument(bytes)

        expect(handle.doc).toBe(doc)
        expect(workers).toHaveLength(1)
        expect(workers[0].options).toMatchObject({ type: "module" })
        expect(workers[0].url).toMatch(/pdf\.worker\.min\.mjs/)
        expect(pdfjs.PDFWorker.create).toHaveBeenCalledWith({ port: expect.any(Object) })
        expect(pdfjs.getDocument).toHaveBeenCalledWith({
            data: bytes,
            worker: pdfWorker,
            BinaryDataFactory: BundledBinaryDataFactory,
            useWorkerFetch: false,
            wasmUrl
        })

        handle.destroy()
        handle.destroy()
        await vi.waitFor(() => expect(workers[0].terminate).toHaveBeenCalledTimes(1))
        expect(task.destroy).toHaveBeenCalledTimes(1)
        expect(pdfWorker.destroy).toHaveBeenCalledTimes(1)
    })

    it("releases the worker when parsing fails", async () => {
        const failure = Object.assign(new Error("No password given"), { name: "PasswordException" })
        pdfjs.getDocument.mockReturnValue(loadingTask(Promise.reject(failure)))

        const opening = openPdfDocument(new Uint8Array())

        await expect(opening).rejects.toBe(failure)
        expect(isPasswordError(await opening.catch((error: unknown) => error))).toBe(true)
        await vi.waitFor(() => expect(workers[0].terminate).toHaveBeenCalledTimes(1))
        expect(pdfWorker.destroy).toHaveBeenCalledTimes(1)
    })

    it("fails the load and terminates a worker that cannot start", async () => {
        const task = { promise: new Promise(() => {}), destroy: vi.fn(() => new Promise(() => {})) }
        pdfjs.getDocument.mockReturnValue(task)

        const opening = openPdfDocument(new Uint8Array())
        await vi.waitFor(() => expect(workers).toHaveLength(1))
        workers[0].target.dispatchEvent(Object.assign(new Event("error"), { message: "SyntaxError" }))

        await expect(opening).rejects.toThrow(/pdf\.js worker failed: SyntaxError/)
        expect(workers[0].terminate).toHaveBeenCalledTimes(1)
        expect(pdfWorker.destroy).toHaveBeenCalledTimes(1)
    })
})

describe("decodeBase64", () => {
    it("round-trips binary bytes", () => {
        const bytes = new Uint8Array([0, 37, 80, 68, 70, 255, 128])
        expect(decodeBase64(btoa(String.fromCharCode(...bytes)))).toEqual(bytes)
    })
})
