import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import i18n from "@/lib/i18n"
import type { SshHostKeyPrompt } from "@/lib/types"
import { SshHostKeyHost } from "@/workbench/SshHostKeyHost"

const ipcMocks = vi.hoisted(() => ({
    sshHostKeyRespond: vi.fn()
}))

let captured: (event: { payload: SshHostKeyPrompt }) => void = () => {}

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_event: string, cb: unknown) => {
        captured = cb as typeof captured
        return () => {}
    })
}))

vi.mock("@/lib/ipc", () => ({
    sshHostKeyRespond: (...args: unknown[]) => ipcMocks.sshHostKeyRespond(...args)
}))

const writeText = vi.fn(async () => undefined)

beforeEach(() => {
    captured = () => {}
    ipcMocks.sshHostKeyRespond.mockReset().mockResolvedValue(undefined)
    writeText.mockReset().mockResolvedValue(undefined)
    Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText }
    })
})

afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
})

const newPrompt: SshHostKeyPrompt = {
    kind: "new",
    challengeId: "chal-1",
    host: "example.com",
    port: 22,
    endpoint: "example.com:22",
    algorithm: "ssh-ed25519",
    fingerprint: "SHA256:abc"
}

const changedPrompt: SshHostKeyPrompt = {
    kind: "changed",
    host: "example.com",
    port: 22,
    endpoint: "example.com:22",
    algorithm: "ssh-ed25519",
    fingerprint: "SHA256:new-key",
    previousFingerprint: "SHA256:old-key"
}

it("shows algorithm and fingerprint and accepts a first-use challenge", async () => {
    render(<SshHostKeyHost />)
    captured({ payload: newPrompt })

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
    expect(screen.getByText("example.com:22")).toBeInTheDocument()
    expect(screen.getByText("ssh-ed25519")).toBeInTheDocument()
    expect(screen.getByTestId("ssh-host-key-fingerprint")).toHaveTextContent("SHA256:abc")
    fireEvent.click(screen.getByRole("button", { name: i18n.t("sshHostKey.accept", { ns: "workbench" }) }))
    await waitFor(() =>
        expect(ipcMocks.sshHostKeyRespond).toHaveBeenCalledWith("chal-1", true, "example.com:22", "SHA256:abc")
    )
})

it("rejects from cancel and Escape", async () => {
    render(<SshHostKeyHost />)
    captured({ payload: newPrompt })
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("sshHostKey.reject", { ns: "workbench" }) }))
    await waitFor(() =>
        expect(ipcMocks.sshHostKeyRespond).toHaveBeenCalledWith("chal-1", false, "example.com:22", "SHA256:abc")
    )

    ipcMocks.sshHostKeyRespond.mockClear()
    captured({ payload: { ...newPrompt, challengeId: "chal-2" } })
    const dialog = await screen.findByRole("alertdialog")
    fireEvent.keyDown(dialog, { key: "Escape" })
    await waitFor(() =>
        expect(ipcMocks.sshHostKeyRespond).toHaveBeenCalledWith("chal-2", false, "example.com:22", "SHA256:abc")
    )
})

it("copies the presented fingerprint", async () => {
    render(<SshHostKeyHost />)
    captured({ payload: newPrompt })
    fireEvent.click(
        await screen.findByRole("button", { name: i18n.t("sshHostKey.copyFingerprint", { ns: "workbench" }) })
    )
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("SHA256:abc"))
})

it("shows old and new fingerprints for a changed key without an accept action", async () => {
    render(<SshHostKeyHost />)
    captured({ payload: changedPrompt })

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
    expect(screen.getByTestId("ssh-host-key-previous")).toHaveTextContent("SHA256:old-key")
    expect(screen.getByTestId("ssh-host-key-fingerprint")).toHaveTextContent("SHA256:new-key")
    expect(
        screen.queryByRole("button", { name: i18n.t("sshHostKey.accept", { ns: "workbench" }) })
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("sshHostKey.close", { ns: "workbench" }) }))
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument())
    expect(ipcMocks.sshHostKeyRespond).not.toHaveBeenCalled()
})
