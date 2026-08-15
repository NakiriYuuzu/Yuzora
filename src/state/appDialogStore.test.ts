import { beforeEach, expect, it } from "vitest"

import {
  requestAppConfirmation,
  showAppMessage,
  useAppDialogStore
} from "@/state/appDialogStore"

beforeEach(() => {
  useAppDialogStore.setState({ pending: null })
})

it("resolves an in-app confirmation from its host response", async () => {
  const result = requestAppConfirmation({
    title: "Close pane",
    description: "Close it?",
    destructive: true
  })
  expect(useAppDialogStore.getState().pending).toMatchObject({
    type: "confirm",
    title: "Close pane"
  })

  useAppDialogStore.getState().respond(true)
  await expect(result).resolves.toBe(true)
})

it("dismisses a superseded confirmation safely", async () => {
  const first = requestAppConfirmation({ title: "First", description: "First" })
  const second = showAppMessage({ title: "Second", description: "Second" })

  await expect(first).resolves.toBe(false)
  useAppDialogStore.getState().respond(true)
  await expect(second).resolves.toBeUndefined()
})
