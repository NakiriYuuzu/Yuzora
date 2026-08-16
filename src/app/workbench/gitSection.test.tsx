import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { GitSection } from "@/app/workbench/GitSection"
import i18n from "@/lib/i18n"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

describe("GitSection localization", () => {
  beforeEach(() => {
    useGitStore.setState(initialGitState)
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    void i18n.changeLanguage("en")
  })

  it.each([
    {
      locale: "en",
      detection: "Detection status",
      unavailable: "Git is currently unavailable. Install it, then run detection again.",
      redetect: "Re-detect",
      remote: "Remote checks",
      probe: "Read-only check",
      interval: "Check interval",
      seconds: "seconds",
    },
    {
      locale: "zh-TW",
      detection: "偵測狀態",
      unavailable: "Git 目前無法使用。安裝完成後，請重新偵測。",
      redetect: "重新偵測",
      remote: "遠端檢查",
      probe: "唯讀檢查",
      interval: "檢查間隔",
      seconds: "秒",
    },
  ])("renders the missing-Git settings surface in $locale without raw diagnostics", async ({
    locale,
    detection,
    unavailable,
    redetect,
    remote,
    probe,
    interval,
    seconds,
  }) => {
    const reason = "git executable was not found on PATH"
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    await i18n.changeLanguage(locale)
    useGitStore.setState({ environment: { status: "missing", reason } })

    render(<GitSection />)

    expect(screen.getByText(detection)).toBeInTheDocument()
    expect(screen.getByText(unavailable)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: redetect })).toBeInTheDocument()
    expect(screen.getAllByText(remote).length).toBeGreaterThan(0)
    expect(screen.getByRole("button", { name: probe })).toBeInTheDocument()
    expect(screen.getByText(interval)).toBeInTheDocument()
    expect(screen.getByText(seconds)).toBeInTheDocument()
    expect(screen.queryByText(reason)).not.toBeInTheDocument()
    expect(warn).toHaveBeenCalledWith("git executable unavailable in settings:", reason)
  })

  it("localizes the not-a-repository state", async () => {
    await i18n.changeLanguage("zh-TW")
    useGitStore.setState({ environment: { status: "notARepo" } })

    render(<GitSection />)

    expect(screen.getByText("目前的工作區不是 Git 儲存庫。")).toBeInTheDocument()
    expect(screen.queryByText("目前的工作區不是 Git repository。")).not.toBeInTheDocument()
  })

  it.each([
    {
      locale: "en",
      title: "Git is too old",
      description: "Yuzora requires Git 2.24 or newer. Upgrade Git, then re-detect.",
      hint: "Install or upgrade to Git 2.24+, then click Re-detect.",
      redetect: "Re-detect",
    },
    {
      locale: "zh-TW",
      title: "Git 版本過舊",
      description: "Yuzora 需要 Git 2.24 或更新版本。請升級 Git 後重新偵測。",
      hint: "請安裝或升級至 Git 2.24+，然後按「重新偵測」。",
      redetect: "重新偵測",
    },
  ])("renders typed unsupported-version guidance in $locale with interpolated 2.24", async ({
    locale,
    title,
    description,
    hint,
    redetect,
  }) => {
    const reason = "git version below 2.24 (requires git switch and --end-of-options): git version 2.23.0"
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    await i18n.changeLanguage(locale)
    useGitStore.setState({
      environment: {
        status: "missing",
        reason,
        kind: "unsupportedVersion",
        minimumVersion: "2.24",
      },
    })

    render(<GitSection />)

    expect(screen.getByText(title)).toBeInTheDocument()
    expect(screen.getByText(description)).toBeInTheDocument()
    expect(screen.getByText(hint)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: redetect })).toBeInTheDocument()
    expect(screen.queryByText(reason)).not.toBeInTheDocument()
    expect(screen.queryByText(/\{version\}/)).not.toBeInTheDocument()
    expect(screen.queryByText("Git is currently unavailable")).not.toBeInTheDocument()
    expect(screen.queryByText("Git 目前無法使用")).not.toBeInTheDocument()
    expect(warn).toHaveBeenCalledWith("git executable unavailable in settings:", reason)
  })
})
