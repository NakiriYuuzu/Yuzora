import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import { ProjectEditorPopover } from "@/app/workbench/ProjectEditorPopover"
import { ProjectNavPanel } from "@/app/workbench/ProjectNavPanel"
import i18n from "@/lib/i18n"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const PATH = "/Users/yuuzu/projects/hanaoka"

beforeEach(async () => {
  await i18n.changeLanguage("en")
  useWorkspaceStore.setState({ workspacePath: PATH })
  useRecentWorkspacesStore.setState({ list: [PATH], presentations: {} })
  useUiStore.setState(uiInitialState)
})

afterEach(() => {
  cleanup()
  useWorkspaceStore.setState({ workspacePath: null })
  useRecentWorkspacesStore.setState({ list: [], presentations: {} })
  useUiStore.setState(uiInitialState)
})

describe("ProjectEditorPopover", () => {
  it("applies name, icon, and color immediately and closing does not revert them", () => {
    useUiStore.getState().openProjectEditor(PATH)
    render(
      <>
        <ProjectNavPanel mode="database" onModeChange={() => {}} onOpenPalette={() => {}} />
        <ProjectEditorPopover />
      </>
    )
    const nav = within(screen.getByLabelText("Project navigation"))

    fireEvent.change(screen.getByRole("textbox", { name: "Project name" }), {
      target: { value: "Studio" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Use ⚡ as project icon" }))
    fireEvent.click(screen.getByRole("button", { name: "Use Ocean as project color" }))

    expect(nav.getByText("Studio")).toBeInTheDocument()
    expect(nav.getByText("⚡")).toBeInTheDocument()
    expect(useRecentWorkspacesStore.getState().presentationFor(PATH)).toEqual({
      name: "Studio",
      glyph: "⚡",
      color: "ocean",
    })

    fireEvent.click(screen.getByRole("button", { name: "Close project editor" }))

    expect(screen.queryByRole("dialog", { name: "Edit project" })).toBeNull()
    expect(nav.getByText("Studio")).toBeInTheDocument()
    expect(useRecentWorkspacesStore.getState().presentationFor(PATH)?.name).toBe("Studio")
  })
})
