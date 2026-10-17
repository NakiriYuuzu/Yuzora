import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { companionPairingDecide } from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"
import { useCompanionStore } from "@/state/companionStore"
import { useOverlayPresence } from "@/state/overlayStore"

/**
 * Global, not inside Settings: a phone may claim after the Settings dialog
 * was closed. Dismissing rejects; only the Approve button approves.
 */
export function CompanionApprovalHost() {
  const { t } = useTranslation("companion")
  const claim = useCompanionStore(s => s.claims[0] ?? null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const settled = useRef<string | null>(null)
  useOverlayPresence(claim !== null)

  if (!claim) return null

  async function decide(decision: "approve" | "reject") {
    if (!claim || settled.current === claim.pairingId) return
    settled.current = claim.pairingId
    setBusy(true)
    setError(null)
    try {
      await companionPairingDecide(claim.pairingId, decision)
      useCompanionStore.getState().dropClaim(claim.pairingId)
    } catch (cause) {
      // A reject that failed still leaves the claim to expire on the gateway.
      if (decision === "reject") useCompanionStore.getState().dropClaim(claim.pairingId)
      else {
        settled.current = null
        setError(describeCompanionError(String(cause), t))
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <AlertDialog open onOpenChange={open => { if (!open && !busy) void decide("reject") }}>
      <AlertDialogContent data-testid="companion-approval-dialog" className="sm:max-w-[440px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("approval.title")}</AlertDialogTitle>
          <AlertDialogDescription>{t("approval.description")}</AlertDialogDescription>
        </AlertDialogHeader>
        <p data-testid="companion-approval-sas" className="text-center font-mono text-[32px] font-semibold tracking-[0.2em]">{claim.sas}</p>
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-[12px]">
          <dt className="text-muted-foreground">{t("approval.device")}</dt>
          <dd className="break-all">{claim.deviceLabel}</dd>
          <dt className="text-muted-foreground">{t("approval.login")}</dt>
          <dd className="break-all">{claim.tailscaleLogin}</dd>
          <dt className="text-muted-foreground">{t("approval.browser")}</dt>
          <dd className="break-all">{claim.userAgent}</dd>
        </dl>
        <p className="text-[12px] text-destructive">{t("approval.warning")}</p>
        {error && <p role="alert" className="text-[12px] text-destructive">{error}</p>}
        <AlertDialogFooter>
          <Button variant="outline" autoFocus disabled={busy} onClick={() => void decide("reject")}>{t("approval.reject")}</Button>
          <Button variant="destructive" disabled={busy} onClick={() => void decide("approve")}>{t("approval.approve")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
