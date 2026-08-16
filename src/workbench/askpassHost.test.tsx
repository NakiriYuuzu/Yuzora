import { expect, it, vi, beforeEach, afterEach } from "vitest"
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react"
import i18n from "@/lib/i18n"
import type { AskpassRequest } from "../lib/types"
import { AskpassHost } from "./AskpassHost"
import * as ipc from "../lib/ipc"

let captured: (e: { payload: AskpassRequest }) => void

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_e: string, cb: unknown) => {
        captured = cb as typeof captured
        return () => {}
    })
}))
vi.mock("../lib/ipc", () => ({ askpassRespond: vi.fn(async () => undefined) }))

function req(
    partial: Partial<AskpassRequest> & Pick<AskpassRequest, "id" | "prompt" | "kind">
): AskpassRequest {
    return {
        repositoryDisplay: "repo",
        repositoryCanonical: "/tmp/repo",
        operation: "fetch",
        remoteDisplay: "origin (git@example.com:foo/bar.git)",
        background: false,
        ...partial
    }
}

beforeEach(async () => {
    captured = () => {}
    await i18n.changeLanguage("en")
})
afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

it("password request shows masked input, trusted context, and responds", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 7, prompt: "Password for 'https://x': ", kind: "password" }) })
    const input = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    expect(input).toHaveAttribute("type", "password")
    expect(screen.getByText(/\/tmp\/repo/)).toBeInTheDocument()
    expect(screen.getByText(i18n.t("askpass.operationFetch", { ns: "workbench" }))).toBeInTheDocument()
    expect(screen.getByText("origin (git@example.com:foo/bar.git)")).toBeInTheDocument()
    expect(screen.getByText(i18n.t("askpass.policyForeground", { ns: "workbench" }))).toBeInTheDocument()
    fireEvent.change(input, { target: { value: "s3cret" } })
    fireEvent.click(screen.getByRole("button", { name: i18n.t("askpass.submit", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(7, "s3cret"))
})

it("username request shows text input and responds on Enter", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 5, prompt: "Username for 'x': ", kind: "username", operation: "pull" }) })
    const input = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    expect(input).toHaveAttribute("type", "text")
    fireEvent.change(input, { target: { value: "octocat" } })
    fireEvent.keyDown(input, { key: "Enter" })
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(5, "octocat"))
})

it("passphrase request shows masked input", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 6, prompt: "Enter passphrase for key: ", kind: "passphrase" }) })
    const input = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    expect(input).toHaveAttribute("type", "password")
})

it("fingerprint request shows full prompt and yes responds", async () => {
    render(<AskpassHost />)
    captured({
        payload: req({
            id: 8,
            prompt: "The authenticity of host...\nSHA256:abc\nAre you sure?",
            kind: "fingerprint",
            operation: "push"
        })
    })
    expect(await screen.findByText(/SHA256:abc/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("askpass.trust", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(8, "yes"))
})

it("cancel responds null", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 9, prompt: "Username for 'x': ", kind: "username" }) })
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("askpass.cancel", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(9, null))
})

it("closing via Esc / onOpenChange responds null", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 10, prompt: "Password: ", kind: "password" }) })
    const input = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    fireEvent.keyDown(input, { key: "Escape" })
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(10, null))
})

it("queues requests and shows them one at a time in order", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 1, prompt: "Username for 'x': ", kind: "username" }) })
    captured({ payload: req({ id: 2, prompt: "Password for 'x': ", kind: "password" }) })

    // First request visible; second is queued (not yet shown).
    const first = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    expect(first).toHaveAttribute("type", "text")

    fireEvent.change(first, { target: { value: "octocat" } })
    fireEvent.click(screen.getByRole("button", { name: i18n.t("askpass.submit", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(1, "octocat"))

    // Second request now shown automatically.
    const second = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    await waitFor(() => expect(second).toHaveAttribute("type", "password"))

    fireEvent.click(screen.getByRole("button", { name: i18n.t("askpass.cancel", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(2, null))
})

it("cancelling a request still shows the next queued request", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 20, prompt: "Username: ", kind: "username" }) })
    captured({ payload: req({ id: 21, prompt: "Host verify\nSHA256:zzz", kind: "fingerprint" }) })

    fireEvent.click(await screen.findByRole("button", { name: i18n.t("askpass.cancel", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(20, null))

    // Next queued fingerprint request appears.
    expect(await screen.findByText(/SHA256:zzz/)).toBeInTheDocument()
})

it("input clears between successive requests", async () => {
    render(<AskpassHost />)
    captured({ payload: req({ id: 30, prompt: "Username: ", kind: "username" }) })
    captured({ payload: req({ id: 31, prompt: "Password: ", kind: "password" }) })

    const first = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    fireEvent.change(first, { target: { value: "leaky" } })
    fireEvent.click(screen.getByRole("button", { name: i18n.t("askpass.submit", { ns: "workbench" }) }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(30, "leaky"))

    const second = await screen.findByLabelText(i18n.t("askpass.inputLabel", { ns: "workbench" }))
    await waitFor(() => expect(second).toHaveValue(""))
})

it("renders backend remote-unknown context without treating prompt as authority", async () => {
    render(<AskpassHost />)
    captured({
        payload: req({
            id: 40,
            prompt: "Password for 'attacker': ",
            kind: "password",
            remoteDisplay: null,
            operation: "probe"
        })
    })
    expect(await screen.findByText(i18n.t("askpass.remoteUnknown", { ns: "workbench" }))).toBeInTheDocument()
    expect(screen.getByText(i18n.t("askpass.operationProbe", { ns: "workbench" }))).toBeInTheDocument()
    expect(screen.getByText(i18n.t("askpass.untrustedPromptNote", { ns: "workbench" }))).toBeInTheDocument()
})
