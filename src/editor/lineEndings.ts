import type { DocumentLineEnding } from "../lib/types"

export type SerializedLineEndings =
    | { kind: "ready"; content: string }
    | { kind: "blocked"; reason: "mixed" }

export function normalizeDocumentLineEndings(content: string): string {
    return content.replace(/\r\n?/g, "\n")
}

export function serializeDocumentLineEndings(
    content: string,
    lineEnding: DocumentLineEnding
): SerializedLineEndings {
    if (lineEnding === "mixed") return { kind: "blocked", reason: "mixed" }

    const normalized = normalizeDocumentLineEndings(content)
    return {
        kind: "ready",
        content: lineEnding === "crlf" ? normalized.replace(/\n/g, "\r\n") : normalized
    }
}
