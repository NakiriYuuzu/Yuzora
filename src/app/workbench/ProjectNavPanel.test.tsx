import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { ProjectNavPanel } from "@/app/workbench/ProjectNavPanel"
import { useWorkspaceStore } from "@/state/workspaceStore"

afterEach(() => {
  cleanup()
  useWorkspaceStore.setState({ workspacePath: null })
})

const renderPanel = () =>
  render(<ProjectNavPanel mode="database" onModeChange={() => {}} onOpenPalette={() => {}} />)

describe("ProjectNavPanel header", () => {
  it("falls back to the Yuzora default when no workspace is open", () => {
    renderPanel()
    expect(screen.getByText("Yuzora")).toBeInTheDocument()
    expect(screen.getByText("~/App/Tauri/yuzora")).toBeInTheDocument()
    expect(screen.getByText("Y")).toBeInTheDocument()
  })

  it("reflects the open workspace's folder name, shortened path, and initial", () => {
    useWorkspaceStore.setState({ workspacePath: "/Users/yuuzu/projects/hanaoka" })
    renderPanel()
    expect(screen.getByText("hanaoka")).toBeInTheDocument()
    expect(screen.getByText("~/projects/hanaoka")).toBeInTheDocument()
    expect(screen.getByText("H")).toBeInTheDocument()
  })
})
