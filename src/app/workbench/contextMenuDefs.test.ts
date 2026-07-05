import { describe, expect, it } from "vitest"

import { CONTEXT_MENU_DEFS, type ContextMenuItem } from "@/app/workbench/contextMenuDefs"
import type { ContextMenuKind } from "@/state/contextMenuStore"

const ids = (kind: ContextMenuKind) =>
  CONTEXT_MENU_DEFS[kind].map((entry) => (entry === "separator" ? "|" : entry.id))

describe("CONTEXT_MENU_DEFS（設計文件 ctxDefs 對照）", () => {
  it("涵蓋全部 11 個區域", () => {
    expect(Object.keys(CONTEXT_MENU_DEFS).sort()).toEqual(
      [
        "agent",
        "editor",
        "explorer",
        "file",
        "general",
        "git",
        "rail",
        "sshhost",
        "status",
        "tab",
        "terminal",
      ].sort()
    )
  })

  it("每個區域的項目序列與設計一致", () => {
    expect(ids("general")).toEqual(["cmCmdPalette", "cmRefresh", "|", "cmSettings", "cmHideSidebar"])
    expect(ids("rail")).toEqual(["cmNewProject", "cmSettings", "|", "cmHideSidebar"])
    expect(ids("explorer")).toEqual(["cmNewFile", "cmNewFolder", "|", "cmRefresh", "cmCopyPath"])
    expect(ids("file")).toEqual([
      "cmOpen",
      "cmOpenSplit",
      "|",
      "cmRename",
      "cmCopyRel",
      "cmReveal",
      "|",
      "cmDelete",
    ])
    expect(ids("tab")).toEqual([
      "cmCloseTab",
      "cmCloseOthers",
      "cmCloseAll",
      "|",
      "cmCopyRel",
      "cmSplit",
    ])
    expect(ids("editor")).toEqual([
      "cmCut",
      "cmCopy",
      "cmPaste",
      "|",
      "cmCompareHead",
      "cmFormatDoc",
      "cmCmdPalette",
    ])
    expect(ids("terminal")).toEqual([
      "cmCopySel",
      "cmPaste",
      "|",
      "cmDockTerm",
      "cmSplitTermRight",
      "cmSplitTermDown",
      "cmClear",
      "cmKill",
    ])
    expect(ids("agent")).toEqual(["cmStop", "cmDuplicate", "cmRenameSession", "|", "cmCopyPath"])
    expect(ids("git")).toEqual([
      "cmStage",
      "cmCopyHash",
      "cmCopyBranch",
      "|",
      "cmFetch",
      "cmPull",
      "cmPush",
      "|",
      "cmRollback",
    ])
    expect(ids("status")).toEqual(["cmCopyBranch", "cmFetch", "cmPull", "cmPush"])
    expect(ids("sshhost")).toEqual(["cmOpenSsh", "cmOpenSftp", "|", "cmCopyAddr", "cmDisconnect"])
  })

  it("danger 項目與設計一致（且只有這四個）", () => {
    const dangerIds = Object.entries(CONTEXT_MENU_DEFS).flatMap(([kind, entries]) =>
      entries
        .filter((entry): entry is ContextMenuItem => entry !== "separator" && entry.danger === true)
        .map((entry) => `${kind}:${entry.id}`)
    )
    expect(dangerIds.sort()).toEqual([
      "file:cmDelete",
      "git:cmRollback",
      "sshhost:cmDisconnect",
      "terminal:cmKill",
    ])
  })
})
