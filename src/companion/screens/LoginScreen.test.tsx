import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import { api } from "../lib/api"
import { storage } from "../store"

const client = vi.hoisted(() => ({ abortLogout: vi.fn(), connect: vi.fn() }))
vi.mock("../client", () => client)
vi.mock("../lib/webauthn", () => ({ getPasskey: vi.fn().mockResolvedValue({ id: "cred" }) }))
import { LoginScreen } from "./LoginScreen"

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.abortLogout.mockReset()
  client.connect.mockReset()
  vi.spyOn(api, "loginOptions").mockResolvedValue({})
  vi.spyOn(storage, "setBinding").mockImplementation(() => undefined)
  vi.spyOn(storage, "setPaired").mockImplementation(() => undefined)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

it("drops an unanswered logout before the new login request is sent", async () => {
  const order: string[] = []
  client.abortLogout.mockImplementation(() => order.push("abort"))
  vi.spyOn(api, "loginFinish").mockImplementation(async () => {
    order.push("finish")
    return { binding: "b", deviceId: "d", scopes: [] }
  })
  render(<I18nextProvider i18n={i18n}><LoginScreen reason="logged-out" /></I18nextProvider>)
  const button = await screen.findByRole("button", { name: /passkey|sign in/i })
  await waitFor(() => expect(button).toBeEnabled())
  fireEvent.click(button)
  await waitFor(() => expect(order).toEqual(["abort", "finish"]))
})
