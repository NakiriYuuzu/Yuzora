import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mockProfileList = vi.fn()
const mockProfileRecover = vi.fn()
const mockProfileImportLegacy = vi.fn()

vi.mock("@/lib/ipc", () => ({
  dbProfileList: (...args: unknown[]) => mockProfileList(...args),
  dbProfileImportLegacy: (...args: unknown[]) => mockProfileImportLegacy(...args),
  dbProfileCreate: vi.fn(),
  dbProfileUpdate: vi.fn(),
  dbProfileForget: vi.fn(),
  dbProfileRemoveCredential: vi.fn(),
  dbProfileRecover: (...args: unknown[]) => mockProfileRecover(...args),
  dbProfileOpen: vi.fn(),
  dbTestConnection: vi.fn(),
  dbList: vi.fn(async () => []),
  dbColumns: vi.fn(async () => []),
  dbQueryRun: vi.fn(),
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}))

import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"
import { DIALOG_SIZE_STORAGE_KEY } from "@/lib/dialogSize"
import { useDbStore } from "@/state/dbStore"

function installLocalStorage(): void {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
      key: (i: number) => [...store.keys()][i] ?? null,
      get length() {
        return store.size
      },
    },
    configurable: true,
    writable: true,
  })
}

function setViewport(width: number, height: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  })
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    writable: true,
    value: height,
  })
  window.dispatchEvent(new Event("resize"))
}

describe("Database recovery dialog layout", () => {
  beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    setViewport(1000, 800)
    useDbStore.getState().reset()
    mockProfileImportLegacy.mockResolvedValue({ profiles: [], recovery: [] })
    mockProfileList.mockResolvedValue({
      profiles: [],
      recovery: [
        {
          operationId: "op-resume-missing",
          descriptorId: "profile-missing",
          kind: "pendingReplace",
          allowedActions: ["resume"],
        },
      ],
    })
    mockProfileRecover
      .mockRejectedValueOnce({ code: "credentialRequired", message: "credential required" })
      .mockResolvedValue({ profiles: [], recovery: [] })
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it("uses a ScrollArea body and shrink-0 footer at the 280×180 minimum", async () => {
    // Force the allowed default minimum so header + description + field cannot fit.
    localStorage.setItem(
      DIALOG_SIZE_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        sizes: { "database-recovery": { widthRatio: 0.01, heightRatio: 0.01 } },
      }),
    )

    render(<DatabaseNavContent />)
    fireEvent.click(await screen.findByText("Resume"))

    const dialog = await screen.findByTestId("database-recovery-dialog")
    expect(dialog).toHaveAttribute("data-dialog-size-id", "database-recovery")
    expect(dialog.style.width).toBe("280px")
    expect(dialog.style.height).toBe("180px")
    expect(dialog.className).toMatch(/min-h-0/)
    expect(dialog.className).toMatch(/flex-col/)
    expect(dialog.className).toMatch(/overflow-hidden/)

    const body = screen.getByTestId("database-recovery-body")
    expect(body).toHaveAttribute("data-slot", "scroll-area")
    expect(body.className).toMatch(/min-h-0/)
    expect(body.className).toMatch(/flex-1/)
    expect(body).toContainElement(within(dialog).getByLabelText("Password"))

    const resume = within(dialog).getByRole("button", { name: "Resume" })
    const cancel = within(dialog).getByRole("button", { name: "Cancel" })
    expect(body.contains(resume)).toBe(false)
    expect(body.contains(cancel)).toBe(false)

    const footer = resume.closest('[data-slot="dialog-footer"]') as HTMLElement
    expect(footer).toBeTruthy()
    expect(footer.className).toMatch(/shrink-0/)
    expect(footer.compareDocumentPosition(body) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy()

    const fieldValue = ["sample", "credential", "value"].join("-")
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: fieldValue },
    })
    fireEvent.click(resume)
    await waitFor(() => expect(mockProfileRecover).toHaveBeenCalledTimes(2))
    const secondCall = mockProfileRecover.mock.calls[1]?.[0] as {
      operationId: string
      action: string
      credential: { password: string } | null
    }
    expect(secondCall).toMatchObject({
      operationId: "op-resume-missing",
      action: "resume",
    })
    expect(secondCall.credential?.password).toBe(fieldValue)
  })
})
