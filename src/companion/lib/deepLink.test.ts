import { describe, expect, it, vi } from "vitest"
import { paneFromMessage, takePaneLink } from "./deepLink"

const link = (ids: unknown) => `#pane=${encodeURIComponent(JSON.stringify(ids))}`

describe("takePaneLink", () => {
  it("reads the pane and strips the link from the address bar", () => {
    const replaceState = vi.fn()
    expect(takePaneLink({ hash: link(["work", "w1:p"]) }, { replaceState })).toEqual({ session: "work", pane: "w1:p" })
    expect(replaceState).toHaveBeenCalledExactlyOnceWith(null, "", "/")
  })

  it("leaves every other address alone", () => {
    const replaceState = vi.fn()
    for (const hash of ["", "#", "#p=abc.def", "#panes=1", "#pane"]) expect(takePaneLink({ hash }, { replaceState })).toBeNull()
    expect(replaceState).not.toHaveBeenCalled()
  })

  it.each([
    ["bad percent-encoding", "#pane=%E0%A4%A"],
    ["text that is not JSON", "#pane=work"],
    ["an empty link", "#pane="],
    ["an object", link({ session: "work", pane: "w1:p" })],
    ["one id", link(["work"])],
    ["three ids", link(["work", "w1:p", "x"])],
    ["ids that are not text", link(["work", 5])],
    ["an empty session", link(["", "w1:p"])],
    ["an empty pane", link(["work", ""])],
    ["an id past the gateway's limit", link(["work", "p".repeat(257)])],
  ])("ignores %s, but still strips it", (_, hash) => {
    const replaceState = vi.fn()
    expect(takePaneLink({ hash }, { replaceState })).toBeNull()
    expect(replaceState).toHaveBeenCalledOnce()
  })
})

describe("paneFromMessage", () => {
  it("reads the service worker's open-pane message", () => {
    expect(paneFromMessage({ type: "yzc-open-pane", session: "work", pane: "w1:p" })).toEqual({ session: "work", pane: "w1:p" })
  })

  it.each([
    ["the update message", "yzc-skip-waiting"],
    ["nothing", null],
    ["another type", { type: "other", session: "work", pane: "w1:p" }],
    ["no pane", { type: "yzc-open-pane", session: "work" }],
    ["ids that are not text", { type: "yzc-open-pane", session: 1, pane: ["w1:p"] }],
    ["an empty session", { type: "yzc-open-pane", session: "", pane: "w1:p" }],
  ])("ignores %s", (_, data) => {
    expect(paneFromMessage(data)).toBeNull()
  })
})
