import { expect, it, vi, beforeEach, afterEach } from "vitest"
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react"
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

beforeEach(() => {
    captured = () => {}
})
afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

it("password request shows masked input and responds", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 7, prompt: "Password for 'https://x': ", kind: "password" } })
    const input = await screen.findByLabelText("認證輸入")
    expect(input).toHaveAttribute("type", "password")
    fireEvent.change(input, { target: { value: "s3cret" } })
    fireEvent.click(screen.getByRole("button", { name: "送出" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(7, "s3cret"))
})

it("username request shows text input and responds on Enter", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 5, prompt: "Username for 'x': ", kind: "username" } })
    const input = await screen.findByLabelText("認證輸入")
    expect(input).toHaveAttribute("type", "text")
    fireEvent.change(input, { target: { value: "octocat" } })
    fireEvent.keyDown(input, { key: "Enter" })
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(5, "octocat"))
})

it("passphrase request shows masked input", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 6, prompt: "Enter passphrase for key: ", kind: "passphrase" } })
    const input = await screen.findByLabelText("認證輸入")
    expect(input).toHaveAttribute("type", "password")
})

it("fingerprint request shows full prompt and yes responds", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 8, prompt: "The authenticity of host...\nSHA256:abc\nAre you sure?", kind: "fingerprint" } })
    expect(await screen.findByText(/SHA256:abc/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "信任並繼續" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(8, "yes"))
})

it("cancel responds null", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 9, prompt: "Username for 'x': ", kind: "username" } })
    fireEvent.click(await screen.findByRole("button", { name: "取消" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(9, null))
})

it("closing via Esc / onOpenChange responds null", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 10, prompt: "Password: ", kind: "password" } })
    const input = await screen.findByLabelText("認證輸入")
    fireEvent.keyDown(input, { key: "Escape" })
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(10, null))
})

it("queues requests and shows them one at a time in order", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 1, prompt: "Username for 'x': ", kind: "username" } })
    captured({ payload: { id: 2, prompt: "Password for 'x': ", kind: "password" } })

    // First request visible; second is queued (not yet shown).
    const first = await screen.findByLabelText("認證輸入")
    expect(first).toHaveAttribute("type", "text")

    fireEvent.change(first, { target: { value: "octocat" } })
    fireEvent.click(screen.getByRole("button", { name: "送出" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(1, "octocat"))

    // Second request now shown automatically.
    const second = await screen.findByLabelText("認證輸入")
    await waitFor(() => expect(second).toHaveAttribute("type", "password"))

    fireEvent.click(screen.getByRole("button", { name: "取消" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(2, null))
})

it("cancelling a request still shows the next queued request", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 20, prompt: "Username: ", kind: "username" } })
    captured({ payload: { id: 21, prompt: "Host verify\nSHA256:zzz", kind: "fingerprint" } })

    fireEvent.click(await screen.findByRole("button", { name: "取消" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(20, null))

    // Next queued fingerprint request appears.
    expect(await screen.findByText(/SHA256:zzz/)).toBeInTheDocument()
})

it("input clears between successive requests", async () => {
    render(<AskpassHost />)
    captured({ payload: { id: 30, prompt: "Username: ", kind: "username" } })
    captured({ payload: { id: 31, prompt: "Password: ", kind: "password" } })

    const first = await screen.findByLabelText("認證輸入")
    fireEvent.change(first, { target: { value: "leaky" } })
    fireEvent.click(screen.getByRole("button", { name: "送出" }))
    await waitFor(() => expect(ipc.askpassRespond).toHaveBeenCalledWith(30, "leaky"))

    const second = await screen.findByLabelText("認證輸入")
    await waitFor(() => expect(second).toHaveValue(""))
})
