import { expect, test } from "vitest"
import { render, screen } from "@testing-library/react"
import i18n from "../lib/i18n"
import { SpecialFileView } from "./SpecialFileView"

test("tooLarge 顯示大小與三個動作", () => {
    render(<SpecialFileView path="/w/big.log" result={{ kind: "tooLarge", size: 99614720 }} />)
    expect(screen.getByText(/95\.0 MB/)).toBeTruthy()
    expect(screen.getByText(i18n.t("specialFileView.revealInSystem", { ns: "panels" }))).toBeTruthy()
    expect(screen.getByText(i18n.t("specialFileView.openExternally", { ns: "panels" }))).toBeTruthy()
    expect(screen.getByText(i18n.t("specialFileView.copyPath", { ns: "panels" }))).toBeTruthy()
})

test("binary 顯示拒開說明", () => {
    render(<SpecialFileView path="/w/a.png" result={{ kind: "binary", size: 1024 }} />)
    expect(screen.getByText(i18n.t("specialFileView.binary", { ns: "panels" }))).toBeTruthy()
})
