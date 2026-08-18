import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { WorkspaceRail } from "@/app/workbench/WorkspaceRail"
import { PROJECT_COLOR_OPTIONS } from "@/app/workbench/projectPresentation"
import { normalizeHerdrSnapshot } from "@/lib/herdrNormalize"
import { canonicalPathKey } from "@/lib/paths"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useUiStore, uiInitialState } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn()
}))

import { open } from "@tauri-apps/plugin-dialog"

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"

const renderRail = () =>
  render(
    <WorkspaceRail
      navCollapsed={false}
      onToggleNav={() => {}}
      onOpenSettings={() => {}}
      terminalOpen={false}
      onToggleTerminalDrawer={() => {}}
    />
  )

function seedSpaces() {
  useHerdrStore.setState({
    ...herdrInitialState,
    attachments: new Map(),
    sessions: [
      {
        name: "default",
        default: true,
        running: true,
        sessionDir: "/tmp/d",
        socketPath: "/tmp/d.sock"
      }
    ],
    selectedSessionName: "default",
    connectionState: "ready",
    selectedSpaceId: "ws-1",
    selectedSpaceBySession: { default: "ws-1" },
    capabilities: {
      binaryPath: "/bin/herdr",
      binarySource: { configured: "global" as const, resolved: "global" as const, available: true, path: "/bin/herdr", reason: null, restartRequired: false },
  server: { running: true },
      api: {
        snapshot: true,
        ping: true,
        tabCreate: true,
        workspaceFocus: true,
        workspaceCreate: true,
        workspaceRename: true,
        workspaceClose: true,
        tabRename: true,
        tabClose: true,
        tabFocus: true,
        paneFocus: true,
        paneRename: true,
        paneSplit: true,
        paneZoom: true,
        paneSwap: true,
        paneClose: true,
        layoutExport: true,
        layoutSetSplitRatio: true,
        agentGet: true,
        agentRead: true,
        eventsSubscribe: true,
        worktreeList: true,
        methods: [
          "session.snapshot",
          "workspace.focus",
          "workspace.create",
          "workspace.rename",
          "workspace.close",
          "tab.create",
          "tab.rename",
          "tab.close",
          "tab.focus",
          "pane.focus",
          "pane.rename",
          "pane.split",
          "pane.zoom",
          "pane.swap",
          "pane.close",
          "layout.export",
          "layout.set_split_ratio"
        ],
        schemaProtocol: 19,
        schemaVersion: 1,
        reason: null
      },
      terminal: {
        observe: true,
        control: true,
        takeover: true,
        input: true,
        resize: true,
        scroll: true,
        release: true,
        create: true,
        reason: null
      },
      events: { status: "deferred" }
    },
    snapshot: {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [
        {
          id: "ws-1",
          label: "Yuzora",
          order: 0,
          focused: true,
          path: "/Users/tester/projects/yuzora",
          status: "working"
        },
        {
          id: "ws-2",
          label: "feature-x",
          order: 1,
          focused: false,
          path: "/Users/tester/projects/feature-x",
          status: "idle"
        }
      ],
      tabs: [
        {
          id: "tab-1",
          label: "Agent",
          order: 1,
          workspaceId: "ws-1",
          paneCount: 2,
          status: "working",
          active: true,
          focused: true,
          terminalId: "term-1",
          sessionName: "default"
        },
        {
          id: "tab-2",
          label: "Shell",
          order: 2,
          workspaceId: "ws-1",
          paneCount: 1,
          status: "idle",
          active: false,
          focused: false,
          terminalId: "term-2",
          sessionName: "default"
        }
      ],
      agents: [
        {
          id: "a1",
          name: "pi",
          status: "working",
          workspaceId: "ws-1",
          terminalId: "term-1"
        },
        {
          id: "a2",
          name: "claude",
          status: "idle",
          workspaceId: "ws-1",
          terminalId: "term-2"
        }
      ],
      terminals: [],
      raw: {}
    }
  })
}

beforeEach(() => {
  useUiStore.setState(uiInitialState)
  useWorkspaceStore.setState({ workspacePath: "/Users/tester/projects/yuzora" })
  useRecentWorkspacesStore.setState({ list: [], presentations: {} })
  useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
  vi.mocked(open).mockReset()
})

afterEach(() => {
  cleanup()
  delete (globalThis as { isTauri?: boolean }).isTauri
  delete (window.navigator as { userAgent?: string }).userAgent
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useRecentWorkspacesStore.setState({ list: [], presentations: {} })
  useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
})

describe("WorkspaceRail 紅綠燈區塊", () => {
  it("不再渲染裝飾圓點或 drag region — 頂部空間由 AppShell 的標題帶統一讓出", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", { value: MAC_UA, configurable: true })

    const { container } = renderRail()

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
    expect(container.querySelector('[class*="ff5f57"]')).toBeNull()
  })
})

it("右鍵 rail 開啟 rail 選單", () => {
  const { container } = renderRail()
  fireEvent.contextMenu(container.querySelector("nav") as HTMLElement)
  expect(useContextMenuStore.getState().request?.kind).toBe("rail")
})

it("不再顯示 Browser／Preview 按鈕", () => {
  renderRail()
  expect(screen.queryByRole("button", { name: "Toggle preview" })).toBeNull()
})

it("以齒輪設定按鈕取代 Y avatar", () => {
  renderRail()
  const settings = screen.getByRole("button", { name: "Settings" })
  expect(settings.querySelector("svg")).not.toBeNull()
  expect(settings).not.toHaveTextContent("Y")
})

describe("WorkspaceRail Spaces list", () => {
  it("enables first-Space creation with workspace.create even when workspace.focus is unavailable", () => {
    seedSpaces()
    const capabilities = useHerdrStore.getState().capabilities!
    useHerdrStore.setState({
      selectedSpaceId: null,
      selectedSpaceBySession: { default: null },
      snapshot: {
        ...useHerdrStore.getState().snapshot!,
        spaces: []
      },
      capabilities: {
        ...capabilities,
        api: { ...capabilities.api, workspaceFocus: false, workspaceCreate: true }
      }
    })

    renderRail()

    expect(screen.getByTestId("rail-new-space")).toBeEnabled()
  })

  it("exposes the first-Space capability reason when creation is disabled", () => {
    seedSpaces()
    const capabilities = useHerdrStore.getState().capabilities!
    useHerdrStore.setState({
      capabilities: {
        ...capabilities,
        api: {
          ...capabilities.api,
          workspaceCreate: false,
          reason: "WSL public control is read-only"
        }
      }
    })

    renderRail()

    const button = screen.getByTestId("rail-new-space")
    expect(button).toBeDisabled()
    expect(button).toHaveAccessibleName("WSL public control is read-only")
  })

  it("uses Compact B geometry with centered tiles and a narrow local scrollbar", () => {
    seedSpaces()
    const { container } = renderRail()
    const rail = container.querySelector("nav") as HTMLElement
    const scrollRoot = screen.getByTestId("rail-spaces-scroll")
    const scrollContent = scrollRoot.querySelector(
      '[data-slot="scroll-area-content"]'
    ) as HTMLElement
    const tile = screen.getByTestId("rail-space-ws-1")
    const glyph = screen.getByTestId("rail-space-glyph-ws-1")
    const label = screen.getByTestId("rail-space-label-ws-1")

    // Exact Compact B dimensions: 68 rail − 58 tile = 5px side margins when centered.
    expect(rail.className).toContain("w-[68px]")
    expect(tile.className).toContain("w-[58px]")
    expect(tile.className).toContain("px-0")
    expect(glyph.className).toContain("size-[34px]")
    expect(label.className).toContain("w-[54px]")
    // Centered content — no asymmetric right padding that would flush tiles left.
    expect(scrollContent.className).toContain("items-center")
    expect(scrollContent.className).not.toContain("pr-[10px]")
    expect(scrollContent.className).not.toMatch(/\bpr-\[/)
    // Local scrollbar stays ≤5px so it fits the right blank margin only.
    expect(scrollRoot.className).toContain(
      "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:w-[5px]"
    )
    expect(scrollRoot.className).toContain(
      "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:p-0"
    )
    expect(scrollRoot.className).toContain(
      "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:border-l-0"
    )
    // Keep keyboard focus ring inside the tile bounds.
    expect(tile.className).toContain("focus-visible:ring-inset")
  })

  it("renders selected session Spaces instead of Recent folders", () => {
    seedSpaces()
    renderRail()
    expect(screen.getByText("Spaces")).toBeInTheDocument()
    expect(screen.queryByText("Recent")).toBeNull()
    expect(screen.getByTestId("rail-space-ws-1")).toBeInTheDocument()
    expect(screen.getByTestId("rail-space-ws-2")).toBeInTheDocument()
    const yuzoraTile = screen.getByRole("button", { name: "Open space Yuzora, 2 tabs" })
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveTextContent("Y")
    expect(screen.getByTestId("rail-space-label-ws-1")).toHaveTextContent("yuzora")
    expect(yuzoraTile).not.toHaveAttribute("title")
  })

  it("uses path-keyed project presentation for glyph, color, and short name", () => {
    seedSpaces()
    const path = "/Users/tester/projects/yuzora"
    useRecentWorkspacesStore.setState({
      presentations: {
        [canonicalPathKey(path)]: {
          name: "Main App",
          glyph: "✦",
          color: "ocean"
        }
      }
    })
    renderRail()

    const ocean = PROJECT_COLOR_OPTIONS.find((option) => option.id === "ocean")
    expect(screen.getByTestId("rail-space-label-ws-1")).toHaveTextContent("Main App")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveTextContent("✦")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveStyle({
      background: ocean?.background,
      color: ocean?.foreground
    })
    // HoverCard metadata remains the raw Herdr Space label/path.
    expect(screen.getByTestId("rail-space-ws-1")).toHaveAccessibleName(
      "Open space Yuzora, 2 tabs"
    )
  })

  it("matches presentation storage through canonical path aliases", () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) =>
              space.id === "ws-1"
                ? { ...space, path: "/Users/tester/projects/yuzora/" }
                : space
            )
          }
        : state.snapshot
    }))
    useRecentWorkspacesStore.setState({
      presentations: {
        [canonicalPathKey("/Users/tester/projects/yuzora")]: {
          name: "Alias App",
          glyph: "A",
          color: "mint"
        }
      }
    })
    renderRail()

    expect(screen.getByTestId("rail-space-label-ws-1")).toHaveTextContent("Alias App")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveTextContent("A")
  })

  it("preserves raw valid path with leading/trailing whitespace for presentation lookup", () => {
    seedSpaces()
    // Operational path may carry trailing whitespace; trim must not rewrite it
    // before canonical lookup / resolveProjectPresentation.
    const rawPath = "/Users/tester/projects/yuzora "
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) =>
              space.id === "ws-1" ? { ...space, path: rawPath } : space
            )
          }
        : state.snapshot
    }))
    useRecentWorkspacesStore.setState({
      presentations: {
        [canonicalPathKey(rawPath)]: {
          name: "Spaced Path App",
          glyph: "★",
          color: "coral"
        }
      }
    })
    const activateSpace = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({ activateSpace })
    renderRail()

    const coral = PROJECT_COLOR_OPTIONS.find((option) => option.id === "coral")
    expect(screen.getByTestId("rail-space-label-ws-1")).toHaveTextContent("Spaced Path App")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveTextContent("★")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveStyle({
      background: coral?.background,
      color: coral?.foreground
    })
    fireEvent.click(screen.getByTestId("rail-space-ws-1"))
    expect(activateSpace).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-1",
      path: rawPath
    })
  })

  it("preserves raw valid path with leading whitespace for presentation lookup", () => {
    seedSpaces()
    // Leading whitespace is also operational path content; never strip it before
    // canonical presentation lookup or activation.
    const rawPath = " /Users/tester/projects/yuzora"
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) =>
              space.id === "ws-1" ? { ...space, path: rawPath } : space
            )
          }
        : state.snapshot
    }))
    useRecentWorkspacesStore.setState({
      presentations: {
        [canonicalPathKey(rawPath)]: {
          name: "Leading Space App",
          glyph: "◆",
          color: "dusk"
        }
      }
    })
    const activateSpace = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({ activateSpace })
    renderRail()

    const dusk = PROJECT_COLOR_OPTIONS.find((option) => option.id === "dusk")
    expect(screen.getByTestId("rail-space-label-ws-1")).toHaveTextContent("Leading Space App")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveTextContent("◆")
    expect(screen.getByTestId("rail-space-glyph-ws-1")).toHaveStyle({
      background: dusk?.background,
      color: dusk?.foreground
    })
    fireEvent.click(screen.getByTestId("rail-space-ws-1"))
    expect(activateSpace).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "ws-1",
      path: rawPath
    })
  })

  it("falls back to Space label/id with neutral identity when path is missing", () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: [
              {
                id: "ws-orphan",
                label: "",
                order: 0,
                focused: false,
                path: null,
                status: "idle"
              },
              ...state.snapshot.spaces
            ]
          }
        : state.snapshot
    }))
    renderRail()

    expect(screen.getByTestId("rail-space-label-ws-orphan")).toHaveTextContent("ws-orphan")
    expect(screen.getByTestId("rail-space-glyph-ws-orphan")).toHaveTextContent("W")
    expect(screen.getByTestId("rail-space-glyph-ws-orphan")).not.toHaveAttribute("style")
  })

  it("uses Space id for visible and accessible identity when label is whitespace-only", () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: [
              {
                id: "ws-blank-label",
                label: "   ",
                order: 0,
                focused: false,
                path: null,
                status: "idle"
              },
              ...state.snapshot.spaces
            ]
          }
        : state.snapshot
    }))
    renderRail()

    expect(screen.getByTestId("rail-space-label-ws-blank-label")).toHaveTextContent(
      "ws-blank-label"
    )
    expect(screen.getByTestId("rail-space-glyph-ws-blank-label")).toHaveTextContent("W")
    expect(screen.getByTestId("rail-space-ws-blank-label")).toHaveAccessibleName(
      "Open space ws-blank-label, 0 tabs"
    )
  })

  it("falls back to S for visible and accessible identity when label and id are blank", () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: [
              {
                id: "   ",
                label: "  ",
                order: 0,
                focused: false,
                path: null,
                status: "idle"
              },
              ...state.snapshot.spaces
            ]
          }
        : state.snapshot
    }))
    renderRail()

    const tile = screen.getByRole("button", { name: "Open space S, 0 tabs" })
    expect(tile).toBeInTheDocument()
    expect(tile).toHaveAccessibleName("Open space S, 0 tabs")
    // Whitespace-only id is part of the testid; query the rendered tile content.
    expect(tile.querySelector("[data-testid^='rail-space-label-']")).toHaveTextContent("S")
    expect(tile.querySelector("[data-testid^='rail-space-glyph-']")).toHaveTextContent("S")
    expect(tile.querySelector("[data-testid^='rail-space-glyph-']")).not.toHaveAttribute("style")
  })

  it("uses explicit Herdr focus as the single active Space", () => {
    seedSpaces()
    useWorkspaceStore.setState({ workspacePath: "/Users/tester/projects/feature-x" })
    renderRail()

    expect(screen.getByTestId("rail-space-ws-1")).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByTestId("rail-space-ws-2")).toHaveAttribute("aria-pressed", "false")
  })

  it("shows agent rollup badge on Spaces", () => {
    seedSpaces()
    renderRail()
    expect(screen.getByText("1/2")).toBeInTheDocument()
  })

  it("uses one resolved tab count for the tile name, badge, and HoverCard", async () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) => ({ ...space, tabCount: 9 }))
          }
        : state.snapshot
    }))
    renderRail()

    const tile = screen.getByTestId("rail-space-ws-1")
    const badge = screen.getByTestId("rail-space-tab-count-ws-1")
    expect(tile).toHaveAccessibleName("Open space Yuzora, 2 tabs")
    expect(tile.getAttribute("aria-label")?.match(/2 tabs/g)).toHaveLength(1)
    expect(badge).toHaveTextContent("2")
    expect(badge).toHaveAccessibleName("2 tabs")
    expect(screen.queryByTestId("rail-space-tab-count-ws-2")).toBeNull()
    expect(screen.getByTestId("rail-space-ws-2")).toHaveAccessibleName(
      "Open space feature-x, 0 tabs"
    )
    expect(screen.getByText("1/2")).toBeInTheDocument()

    tile.focus()
    const card = await screen.findByTestId("rail-space-card-ws-1")
    expect(card).toHaveTextContent("2 terminal tabs")
    expect(card).not.toHaveTextContent("9 terminal tabs")
  })

  it("shares the normalized empty-topology tabCount fallback across accessible and visible output", async () => {
    seedSpaces()
    useHerdrStore.setState({
      snapshot: normalizeHerdrSnapshot(
        {
          protocol: 19,
          version: "0.8.0",
          snapshot: {
            workspaces: [
              {
                workspace_id: "ws-1",
                label: "Yuzora",
                number: 0,
                focused: true,
                path: "/Users/tester/projects/yuzora",
                agent_status: "working",
                tab_count: 4
              }
            ]
          }
        },
        "default"
      )
    })
    renderRail()

    const tile = screen.getByTestId("rail-space-ws-1")
    const badge = screen.getByTestId("rail-space-tab-count-ws-1")
    expect(tile).toHaveAccessibleName("Open space Yuzora, 4 tabs")
    expect(badge).toHaveTextContent("4")
    expect(badge).toHaveAccessibleName("4 tabs")

    tile.focus()
    expect(await screen.findByTestId("rail-space-card-ws-1")).toHaveTextContent(
      "4 terminal tabs"
    )
  })

  it("activates Space with the raw path and preserves HoverCard metadata", async () => {
    seedSpaces()
    const activateSpace = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({ activateSpace })
    renderRail()

    fireEvent.click(screen.getByTestId("rail-space-ws-2"))
    await waitFor(() => {
      expect(activateSpace).toHaveBeenCalledWith({
        sessionName: "default",
        workspaceId: "ws-2",
        path: "/Users/tester/projects/feature-x"
      })
    })

    const tile = screen.getByTestId("rail-space-ws-1")
    tile.focus()
    expect(tile).toHaveFocus()
    expect(await screen.findByTestId("rail-space-card-ws-1")).toBeInTheDocument()
    expect(screen.getByTestId("rail-space-card-label-ws-1")).toHaveTextContent("Yuzora")
    expect(screen.getByTestId("rail-space-card-path-ws-1")).toHaveTextContent(
      "/Users/tester/projects/yuzora"
    )
  })

  it("keeps settings and + new space controls", () => {
    seedSpaces()
    renderRail()
    expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument()
    expect(screen.getByTestId("rail-new-space")).toBeInTheDocument()
  })

  it("folder picker + creates Herdr Space via single store transaction", async () => {
    seedSpaces()
    const createSpaceFromFolder = vi.fn().mockResolvedValue({
      ok: true,
      space: {
        id: "ws-new",
        label: "new",
        order: 2,
        focused: true,
        path: "/tmp/new"
      }
    })
    useHerdrStore.setState({
      createSpaceFromFolder
    })
    vi.mocked(open).mockResolvedValue("/tmp/new")

    renderRail()
    fireEvent.click(screen.getByTestId("rail-new-space"))
    await waitFor(() => {
      expect(createSpaceFromFolder).toHaveBeenCalledWith("/tmp/new", "new")
    })
  })

  it("disables new space but keeps stopped Space tiles focusable for disclosure and menus", async () => {
    seedSpaces()
    useHerdrStore.setState({
      connectionState: "stopped",
      sessions: [
        {
          name: "default",
          default: true,
          running: false,
          sessionDir: "/tmp/d",
          socketPath: "/tmp/d.sock"
        }
      ]
    })
    renderRail()
    expect(screen.getByTestId("rail-new-space")).toBeDisabled()
    const tile = screen.getByTestId("rail-space-ws-1")
    expect(tile).not.toBeDisabled()
    expect(tile).toHaveAttribute("aria-disabled", "true")
    tile.focus()
    expect(tile).toHaveFocus()
    expect(await screen.findByTestId("rail-space-card-ws-1")).toBeInTheDocument()
    fireEvent.contextMenu(tile)
    expect(useContextMenuStore.getState().request).toMatchObject({
      kind: "herdrSpace",
      sessionName: "default",
      workspaceId: "ws-1"
    })
  })
})

describe("WorkspaceRail worktree provenance", () => {
  it("keeps a flat Rail and exposes source vs linked provenance in ARIA/tooltip", async () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) =>
              space.id === "ws-1"
                ? {
                    ...space,
                    isLinkedWorktree: false,
                    branch: "main",
                    repoName: "yuzora",
                    repoRoot: "/Users/tester/projects/yuzora",
                    sourceCheckoutPath: "/Users/tester/projects/yuzora"
                  }
                : {
                    ...space,
                    isLinkedWorktree: true,
                    branch: "feature/x",
                    repoName: "yuzora",
                    path: "/Users/tester/projects/yuzora-feature"
                  }
            )
          }
        : null
    }))
    renderRail()
    // Still a single flat list — no nested worktree tree.
    expect(screen.getByTestId("rail-spaces-scroll").querySelectorAll("[data-testid^='rail-space-']").length).toBeGreaterThanOrEqual(2)
    expect(screen.queryByRole("tree")).toBeNull()
    const source = screen.getByTestId("rail-space-ws-1")
    expect(source).toHaveAttribute("data-worktree-kind", "source")
    expect(source).toHaveAccessibleName(/source checkout/i)
    const linked = screen.getByTestId("rail-space-ws-2")
    expect(linked).toHaveAttribute("data-worktree-kind", "linked")
    expect(linked).toHaveAccessibleName(/linked worktree/i)

    fireEvent.pointerEnter(source)
    expect(await screen.findByTestId("rail-space-card-provenance-ws-1")).toHaveTextContent(
      /Source checkout/i
    )
    expect(screen.getByTestId("rail-space-card-provenance-ws-1")).toHaveTextContent(/main/)
  })

  it("announces a linked non-detached null branch as no branch", () => {
    seedSpaces()
    useHerdrStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            spaces: state.snapshot.spaces.map((space) =>
              space.id === "ws-2"
                ? {
                    ...space,
                    isLinkedWorktree: true,
                    isDetached: false,
                    branch: null,
                    repoName: "yuzora"
                  }
                : space
            )
          }
        : null
    }))
    renderRail()

    expect(screen.getByTestId("rail-space-ws-2")).toHaveAccessibleName(
      /linked worktree, No branch/i
    )
    expect(screen.getByTestId("rail-space-ws-2")).not.toHaveAccessibleName(
      /Detached HEAD/i
    )
  })
})
