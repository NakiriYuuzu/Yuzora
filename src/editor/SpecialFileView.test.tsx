import { expect, test } from "vitest"
import { render, screen } from "@testing-library/react"
import { SpecialFileView } from "./SpecialFileView"

test("tooLarge 顯示大小與三個動作", () => {
    render(<SpecialFileView path="/w/big.log" result={{ kind: "tooLarge", size: 99614720 }} />)
    expect(screen.getByText(/95\.0 MB/)).toBeTruthy()
    expect(screen.getByText("在系統中顯示")).toBeTruthy()
    expect(screen.getByText("以外部程式開啟")).toBeTruthy()
    expect(screen.getByText("複製路徑")).toBeTruthy()
})

test("binary 顯示拒開說明", () => {
    render(<SpecialFileView path="/w/a.png" result={{ kind: "binary", size: 1024 }} />)
    expect(screen.getByText(/二進位檔案/)).toBeTruthy()
})
