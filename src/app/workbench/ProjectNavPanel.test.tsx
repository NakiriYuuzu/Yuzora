import { afterEach, describe, expect, it } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { ProjectNavPanel } from "@/app/workbench/ProjectNavPanel"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

afterEach(() => {
  cleanup()
  useWorkspaceStore.setState({ workspacePath: null })
  useRecentWorkspacesStore.setState({ list: [], presentations: {} })
  useUiStore.setState(uiInitialState)
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

  it("presents an extended Windows workspace without exposing its prefix", () => {
    useWorkspaceStore.setState({ workspacePath: "\\\\?\\C:\\Users\\Yuuzu\\專案 空間" })
    renderPanel()

    expect(screen.getByText("專案 空間")).toBeInTheDocument()
    expect(screen.getByText("C:\\Users\\Yuuzu\\專案 空間")).toBeInTheDocument()
    expect(screen.queryByText(/\\\\\?\\/)).toBeNull()
    expect(screen.getByText("專")).toBeInTheDocument()
  })

  it("uses the current workspace's saved project presentation", () => {
    const path = "/Users/yuuzu/projects/hanaoka"
    useWorkspaceStore.setState({ workspacePath: path })
    useRecentWorkspacesStore.setState({
      presentations: {
        [path]: { name: "Studio", glyph: "🧩", color: "ocean" }
      }
    })

    renderPanel()

    expect(screen.getByText("Studio")).toBeInTheDocument()
    expect(screen.getByText("🧩")).toBeInTheDocument()
  })

  it("opens the same project editor from the header chevron", () => {
    const path = "/Users/yuuzu/projects/hanaoka"
    useWorkspaceStore.setState({ workspacePath: path })
    renderPanel()

    fireEvent.click(screen.getByRole("button", { name: "Edit project hanaoka" }))

    expect(useUiStore.getState().projectEditorPath).toBe(path)
  })
})
