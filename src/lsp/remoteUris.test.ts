import { describe, expect, it } from "vitest"
import { mapRemoteLspMessage } from "./remoteUris"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { pathToUri, uriToPath } from "./workspace"

describe("host-owned LSP URIs", () => {
  it("does not share diagnostics or edits between overlapping workspaces", () => {
    const outer = remoteFilePath("same-host", "/repo")
    const inner = remoteFilePath("same-host", "/repo/sub")
    const message = JSON.stringify({ params: { uri: "file:///repo/sub/shared.ts" } })
    const outerMessage = mapRemoteLspMessage(message, outer, "fromHost")
    const innerMessage = mapRemoteLspMessage(message, inner, "fromHost")
    expect(outerMessage).not.toBe(innerMessage)
    expect(() => mapRemoteLspMessage(innerMessage, outer, "toHost")).toThrow("another workspace")
    expect(() => mapRemoteLspMessage(message, inner, "toHost")).toThrow("requires workspace identity")
    expect(mapRemoteLspMessage(innerMessage, inner, "toHost")).toBe(message)
  })
  it("maps deprecated rootPath to a native host path without rewriting document text", () => {
    const request = { params: { rootPath: remoteFilePath("a", "/中文 folder"), text: "/unchanged" } }
    const host = JSON.parse(mapRemoteLspMessage(JSON.stringify(request), remoteFilePath("a", "/中文 folder"), "toHost"))
    expect(host.params.rootPath).toBe("/中文 folder")
    expect(JSON.parse(mapRemoteLspMessage(JSON.stringify(host), remoteFilePath("a", "/中文 folder"), "fromHost"))).toEqual(request)
    expect(() => mapRemoteLspMessage(JSON.stringify(request), remoteFilePath("b", "/repo"), "toHost")).toThrow("another host")
  })
  it("keeps editor identity and source text while mapping protocol URI fields", () => {
    const uri = remoteFilePath("host-a", "/中文 folder/a#%.ts", "/中文 folder")
    expect(pathToUri(uri)).toBe(uri)
    expect(uriToPath(uri)).toBe(uri)
    const request = { jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, text: uri } } }
    const host = JSON.parse(mapRemoteLspMessage(JSON.stringify(request), remoteFilePath("host-a", "/中文 folder"), "toHost"))
    expect(host.params.textDocument.uri).toBe("file:///%E4%B8%AD%E6%96%87%20folder/a%23%25.ts")
    expect(host.params.textDocument.text).toBe(uri)
    const projected = JSON.parse(mapRemoteLspMessage(JSON.stringify(host), remoteFilePath("host-a", "/中文 folder"), "fromHost"))
    expect(projected).toEqual(request)
  })
  it("maps workspace edits and refuses a document from another host", () => {
    const edit = { changes: { "file:///repo/a.ts": [{ newText: "file:///unchanged-text" }] }, documentChanges: [{ oldUri: "file:///repo/b.ts", newUri: "file:///repo/c.ts" }] }
    const projected = JSON.parse(mapRemoteLspMessage(JSON.stringify(edit), remoteFilePath("b", "/repo"), "fromHost"))
    expect(projected.changes[remoteFilePath("b", "/repo/a.ts", "/repo")][0].newText).toBe("file:///unchanged-text")
    expect(projected.documentChanges[0].newUri).toBe(remoteFilePath("b", "/repo/c.ts", "/repo"))
    expect(() => mapRemoteLspMessage(JSON.stringify({ uri: remoteFilePath("a", "/repo/a.ts") }), remoteFilePath("b", "/repo"), "toHost")).toThrow("another host")
  })
})
