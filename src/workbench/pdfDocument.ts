import type { PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs"
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url"

import { openFile, readFileBase64 } from "@/lib/ipc"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"

// Local reads share open_file's 50 MiB hard cap; base64 over IPC already
// inflates that by a third. Remote reads go through the host helper or SFTP,
// both bounded by the host protocol's MAX_FILE_BYTES.
export const LOCAL_PDF_MAX_BYTES = 50 * 1024 * 1024
export const REMOTE_PDF_MAX_BYTES = 8 * 1024 * 1024

export type PdfBytesResult =
    | { kind: "ok"; bytes: Uint8Array; size: number }
    | { kind: "tooLarge"; size: number | null; limit: number }

export interface PdfDocumentHandle {
    doc: PDFDocumentProxy
    destroy: () => void
}

export function pdfByteLimit(path: string): number {
    return parseRemoteFilePath(path) ? REMOTE_PDF_MAX_BYTES : LOCAL_PDF_MAX_BYTES
}

export function decodeBase64(data: string): Uint8Array {
    const binary = atob(data)
    const bytes = new Uint8Array(binary.length)
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
    return bytes
}

// Local: "file too large: N bytes (max M)"; host helper and SFTP: "file-too-large".
function isTooLargeError(error: unknown): boolean {
    return /too[\s-]large/i.test(String(error))
}

export function isPasswordError(error: unknown): boolean {
    return error instanceof Error && error.name === "PasswordException"
}

export function isRenderCancelled(error: unknown): boolean {
    return error instanceof Error && error.name === "RenderingCancelledException"
}

export async function readPdfBytes(path: string): Promise<PdfBytesResult> {
    const limit = pdfByteLimit(path)
    try {
        const file = await readFileBase64(path, limit)
        return { kind: "ok", bytes: decodeBase64(file.data), size: file.size }
    } catch (error) {
        if (!isTooLargeError(error)) throw error
        // The size only feeds the message; SFTP reports none.
        const size = await openFile(path).then((result) => result.size, () => null)
        return { kind: "tooLarge", size, limit }
    }
}

let pdfjsModule: Promise<typeof import("pdfjs-dist/legacy/build/pdf.mjs")> | null = null

// The legacy build: the generic one needs Safari 18-level APIs (URL.parse,
// Response.bytes) that older WKWebView / WebKitGTK releases lack.
function loadPdfJs() {
    pdfjsModule ??= import("pdfjs-dist/legacy/build/pdf.mjs").catch((error: unknown) => {
        pdfjsModule = null
        throw error
    })
    return pdfjsModule
}

/** Parses PDF bytes on a dedicated worker; `destroy` releases both. */
export async function openPdfDocument(bytes: Uint8Array): Promise<PdfDocumentHandle> {
    const [pdfjs, { BundledBinaryDataFactory, wasmUrl }] = await Promise.all([loadPdfJs(), import("./pdfResources")])
    // pdf.js sees tauri://localhost as cross-origin (its URL origin is "null")
    // and would wrap workerSrc in a blob: URL that CSP worker-src 'self'
    // rejects, silently falling back to parsing on the main thread. Hand it a
    // same-origin module worker instead.
    const worker = new Worker(workerUrl, { type: "module", name: "pdfjs" })
    const pdfWorker = pdfjs.PDFWorker.create({ port: worker })
    const task = pdfjs.getDocument({
        data: bytes,
        worker: pdfWorker,
        BinaryDataFactory: BundledBinaryDataFactory,
        useWorkerFetch: false,
        wasmUrl
    })
    // With a supplied port pdf.js never hears about a worker that fails to
    // start, and task.destroy() would wait on it forever: fail the load and
    // terminate directly instead.
    let workerFailed = false
    const failed = new Promise<never>((_, reject) => {
        worker.addEventListener("error", (event) => {
            workerFailed = true
            // A module that fails to load fires a plain Event without a message.
            const message = (event as { message?: string }).message
            reject(new Error(`pdf.js worker failed: ${message || "could not start"}`))
        }, { once: true })
    })
    failed.catch(() => {})
    let released = false
    const destroy = () => {
        if (released) return
        released = true
        const release = () => {
            pdfWorker.destroy()
            worker.terminate()
        }
        if (workerFailed) {
            release()
            void task.destroy().catch(() => {})
        } else void task.destroy().catch(() => {}).then(release)
    }
    try {
        return { doc: await Promise.race([task.promise, failed]), destroy }
    } catch (error) {
        destroy()
        throw error
    }
}
