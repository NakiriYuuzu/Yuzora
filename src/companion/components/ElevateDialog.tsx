import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { endSession } from "../client"
import { api, ApiError } from "../lib/api"
import { failureKey } from "../lib/errors"
import { getPasskey } from "../lib/webauthn"
import { storage, useCompanion } from "../store"
import { ConfirmDialog } from "./ConfirmDialog"
import { Message } from "./Page"

/** Why a phone cannot elevate, from the options request's refusal. */
function refusalKey(error: unknown): string {
  if (error instanceof ApiError && error.status === 403 && error.body === "Scope not granted") return "rpc.scope-not-granted"
  return failureKey(error)
}

/**
 * A fresh passkey assertion for control. Mount it only while shown: the
 * options are fetched on mount so the confirm tap can start the ceremony
 * directly, as Safari requires.
 */
export function ElevateDialog({ title, description, confirmLabel, onElevated, onCancel }: {
  title: string
  description: string
  confirmLabel: string
  onElevated: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [options, setOptions] = useState<unknown>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [generation, setGeneration] = useState(0)

  useEffect(() => {
    const binding = storage.binding()
    if (!binding) return
    let live = true
    api.elevateOptions("control", binding).then(
      (value) => live && setOptions(value),
      (failure: unknown) => {
        if (!live) return
        if (failure instanceof ApiError && failure.status === 401) return endSession("expired")
        setError(refusalKey(failure))
      },
    )
    return () => {
      live = false
    }
  }, [generation])

  const confirm = async () => {
    const binding = storage.binding()
    if (!options || !binding) return
    const current = options
    setOptions(null)
    setBusy(true)
    setError(null)
    try {
      const elevated = await api.elevateFinish("control", await getPasskey(current), binding)
      useCompanion.setState({ elevation: { scope: elevated.scope, until: elevated.until, at: Date.now() } })
      onElevated()
    } catch (failure) {
      setError(failureKey(failure, "control.elevateRejected"))
      setGeneration((value) => value + 1)
    } finally {
      setBusy(false)
    }
  }

  return (
    <ConfirmDialog title={title} description={description} confirmLabel={confirmLabel} busy={busy} disabled={!options} onConfirm={() => void confirm()} onCancel={onCancel}>
      {error && <Message tone="error">{t(error)}</Message>}
    </ConfirmDialog>
  )
}
