import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { companionPairingCreate, companionPairingDecide } from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"
import { useNow } from "@/lib/useNow"
import { useCompanionStore } from "@/state/companionStore"

const cancel = (pairingId: string) => void companionPairingDecide(pairingId, "cancel").catch(() => undefined)

/**
 * One pairing per mount. The QR encodes the pairing secret, so it lives in
 * this component's state only. Closing cancels the pairing, unless a phone
 * claimed it: the approval dialog takes over from there.
 */
export function CompanionPairingDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation("companion")
  // Only a blob URL is kept: the SVG text is dropped once it is wrapped.
  const [pairing, setPairing] = useState<{ pairingId: string; expiresAt: number; url: string } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const handedOff = useRef(false)
  const now = useNow(1000)
  const pairingId = pairing?.pairingId ?? null
  const done = useCompanionStore(s => pairingId !== null && (pairingId in s.settled || s.claims.some(claim => claim.pairingId === pairingId)))
  const expired = pairing !== null && now >= pairing.expiresAt

  useEffect(() => {
    let active = true
    let created: { pairingId: string; url: string } | null = null
    void companionPairingCreate().then(next => {
      if (!active) return cancel(next.pairingId)
      created = { pairingId: next.pairingId, url: URL.createObjectURL(new Blob([next.qrSvg], { type: "image/svg+xml" })) }
      setPairing({ ...created, expiresAt: Date.parse(next.expiresAt) })
    }).catch(cause => { if (active) setError(String(cause)) })
    // Each run revokes what it created, so StrictMode cannot leak a URL.
    return () => {
      active = false
      if (!created) return
      URL.revokeObjectURL(created.url)
      if (!handedOff.current) cancel(created.pairingId)
    }
  }, [])

  useEffect(() => {
    if (done) handedOff.current = true
    if (done || expired) onClose()
  }, [done, expired, onClose])

  const remaining = pairing ? Math.max(0, Math.ceil((pairing.expiresAt - now) / 1000)) : 0
  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent data-testid="companion-pairing-dialog" className="sm:max-w-[380px]">
        <DialogHeader>
          <DialogTitle>{t("pairing.title")}</DialogTitle>
          <DialogDescription>{t("pairing.description")}</DialogDescription>
        </DialogHeader>
        {error ? <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(error, t)}</p>
          : pairing ? <div className="flex flex-col items-center gap-2">
            <img src={pairing.url} alt={t("pairing.qrAlt")} className="size-[240px] rounded-md bg-white p-2" />
            <p role="timer" className="text-[12px] text-muted-foreground">
              {t("pairing.expiresIn", { time: `${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, "0")}` })}
            </p>
          </div>
          : <p className="text-[12px] text-muted-foreground">{t("pairing.creating")}</p>}
      </DialogContent>
    </Dialog>
  )
}
