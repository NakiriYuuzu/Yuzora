import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { CompanionPairingDialog } from "@/app/workbench/CompanionPairingDialog"
import { CompanionPhoneLinkDialog } from "@/app/workbench/CompanionPhoneLinkDialog"
import { describeCompanionError } from "@/lib/companionErrors"
import { useCompanionStore } from "@/state/companionStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useUiStore } from "@/state/uiStore"

/**
 * The greeting after the user enabled the Companion (D13). Global, not inside
 * Settings: enabling can take a minute, and the dialog may have closed by then.
 * The first phone has to pair; later ones only need the address. An enable that
 * fails after Settings closed is told here too, since no card is left to show it.
 */
export function CompanionWelcomeHost() {
  const { t } = useTranslation("companion")
  const [pairing, setPairing] = useState(false)
  const [linking, setLinking] = useState(false)
  const closePairing = useCallback(() => setPairing(false), [])
  const closeLinking = useCallback(() => setLinking(false), [])
  useOverlayPresence(pairing || linking)

  useEffect(() => useCompanionStore.subscribe((state, previous) => {
    // While Settings is open the card shows the failure inline: never both.
    if (state.enableError && state.enableError !== previous.enableError && state.welcomePending && !useUiStore.getState().settingsOpen) {
      toast.error(describeCompanionError(state.enableError, t, state.overview?.settings.gatewayPort))
    }
    // The listener is open, not only the gateway: the QR it shows must lead somewhere.
    if (!state.welcomePending || state.overview?.status.state !== "running" || !state.overview.net.serving) return
    state.setWelcomePending(false)
    if (state.devices.length === 0) setPairing(true)
    else toast(t("enable.ready"), { action: { label: t("phoneLink.open"), onClick: () => setLinking(true) } })
  }), [t])

  return <>
    {pairing && <CompanionPairingDialog onClose={closePairing} />}
    {linking && <CompanionPhoneLinkDialog onClose={closeLinking} />}
  </>
}
