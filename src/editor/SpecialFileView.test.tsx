import { afterEach, beforeEach, expect, test, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import i18n from "../lib/i18n"
import { remoteFilePath } from "../lib/runtimeIdentity"
import { useAppDialogStore } from "../state/appDialogStore"

const openFileExternally = vi.hoisted(() => vi.fn())
const nativeOpenPath = vi.hoisted(() => vi.fn())
vi.mock("../lib/revealPath", () => ({ openFileExternally }))
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn(), openPath: nativeOpenPath }))

import { SpecialFileView } from "./SpecialFileView"

const label = (key: string) => i18n.t(key, { ns: "panels" })

beforeEach(() => {
    openFileExternally.mockReset().mockResolvedValue(undefined)
    nativeOpenPath.mockReset()
    useAppDialogStore.setState({ pending: null })
})

afterEach(() => cleanup())

test("tooLarge 顯示大小與三個動作", () => {
    render(<SpecialFileView path="/w/big.log" result={{ kind: "tooLarge", size: 99614720 }} />)
    expect(screen.getByText(/95\.0 MB/)).toBeTruthy()
    expect(screen.getByText(label("specialFileView.revealInSystem"))).toBeTruthy()
    expect(screen.getByText(label("specialFileView.openExternally"))).toBeTruthy()
    expect(screen.getByText(label("specialFileView.copyPath"))).toBeTruthy()
})

test("binary 顯示拒開說明", () => {
    render(<SpecialFileView path="/w/a.png" result={{ kind: "binary", size: 1024 }} />)
    expect(screen.getByText(label("specialFileView.binary"))).toBeTruthy()
})

test("以外部程式開啟走受控入口，不直接呼叫 opener plugin", () => {
    render(<SpecialFileView path="/w/a.pdf" result={{ kind: "binary", size: 1024 }} />)
    fireEvent.click(screen.getByText(label("specialFileView.openExternally")))
    expect(openFileExternally).toHaveBeenCalledWith("/w/a.pdf")
    expect(nativeOpenPath).not.toHaveBeenCalled()
    expect(useAppDialogStore.getState().pending).toBeNull()
})

test("開啟失敗時顯示原因，而不是留下無人處理的 rejection", async () => {
    openFileExternally.mockRejectedValueOnce("Only files in the current workspace can be opened")
    render(<SpecialFileView path="/w/a.pdf" result={{ kind: "binary", size: 1024 }} />)
    fireEvent.click(screen.getByText(label("specialFileView.openExternally")))
    await vi.waitFor(() => expect(useAppDialogStore.getState().pending).toMatchObject({ type: "message", kind: "error" }))
    expect(useAppDialogStore.getState().pending?.description).toContain("Only files in the current workspace can be opened")
})

test("遠端檔案明確告知不支援以本機程式開啟，且不呼叫本機入口", async () => {
    render(<SpecialFileView path={remoteFilePath("ssh-host", "/srv/a.zip")} result={{ kind: "binary", size: 1024 }} />)
    fireEvent.click(screen.getByText(label("specialFileView.openExternally")))
    await vi.waitFor(() => expect(useAppDialogStore.getState().pending).toMatchObject({ type: "message", kind: "error" }))
    expect(useAppDialogStore.getState().pending?.description).toContain(label("specialFileView.remoteOpenUnsupported"))
    expect(openFileExternally).not.toHaveBeenCalled()
})
