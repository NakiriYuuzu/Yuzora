import { describe, it, expect, vi, beforeEach } from "vitest"

// assembleLspExtensions is pure composition, so mock each piece it wires and
// assert the capability gate (a pull-mode linter only for a server advertising a
// diagnosticProvider) plus the always-on semanticTokens — no live LSP client
// needed. The push-mode serverDiagnostics channel is an LSPClientExtension wired
// on the client itself (lspManager), not here, so its wiring is covered in
// lspManager.test.ts. Distinct marker strings stand in for each extension so array
// membership is assertable.
const languageServerSupport = vi.fn((_client: unknown, _uri: string) => "LSS")
vi.mock("@codemirror/lsp-client", () => ({
    languageServerSupport: (client: unknown, uri: string) => languageServerSupport(client, uri)
}))

const linter = vi.fn((_source: unknown) => "LINTER")
vi.mock("@codemirror/lint", () => ({
    linter: (source: unknown) => linter(source)
}))

const lspLintSource = vi.fn((_client: unknown, _uri: string) => "LINTSOURCE")
vi.mock("./codeActions", () => ({
    lspLintSource: (client: unknown, uri: string) => lspLintSource(client, uri)
}))

const semanticTokensExtension = vi.fn((_client: unknown, _uri: string) => "ST")
vi.mock("./semanticTokens", () => ({
    semanticTokensExtension: (client: unknown, uri: string) => semanticTokensExtension(client, uri)
}))

vi.mock("./workspace", () => ({
    pathToUri: (p: string) => "file://" + p
}))

const { assembleLspExtensions } = await import("./lspExtensions")

function managedWith(serverCapabilities: object) {
    return {
        client: { serverCapabilities },
        language: "typescript" as const,
        capabilities: null
    }
}

beforeEach(() => {
    vi.clearAllMocks()
})

describe("assembleLspExtensions diagnostics wiring", () => {
    it("adds the pull-mode linter (with quick-fix) only when the server advertises a diagnosticProvider", () => {
        const ext = assembleLspExtensions(managedWith({ diagnosticProvider: {} }) as never, "/w/a.ts")
        expect(lspLintSource).toHaveBeenCalledWith(expect.anything(), "file:///w/a.ts")
        expect(linter).toHaveBeenCalledWith("LINTSOURCE")
        expect(ext).toContain("LINTER")
    })

    it("omits the linter for a push-only server (no diagnosticProvider) so no doomed pulls are sent", () => {
        const ext = assembleLspExtensions(managedWith({}) as never, "/w/a.ts")
        expect(lspLintSource).not.toHaveBeenCalled()
        expect(linter).not.toHaveBeenCalled()
        expect(ext).not.toContain("LINTER")
    })

    it("always wires semanticTokens and the LSP baseline, provider or not", () => {
        const withPull = assembleLspExtensions(managedWith({ diagnosticProvider: {} }) as never, "/w/a.ts")
        expect(semanticTokensExtension).toHaveBeenCalled()
        expect(languageServerSupport).toHaveBeenCalled()
        expect(withPull).toContain("ST")
        expect(withPull).toContain("LSS")

        vi.clearAllMocks()

        const pushOnly = assembleLspExtensions(managedWith({}) as never, "/w/a.ts")
        expect(semanticTokensExtension).toHaveBeenCalled()
        expect(languageServerSupport).toHaveBeenCalled()
        expect(pushOnly).toContain("ST")
        expect(pushOnly).toContain("LSS")
    })
})
