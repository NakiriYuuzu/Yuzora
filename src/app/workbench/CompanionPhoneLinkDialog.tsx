import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { companionPhoneLink } from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"

/**
 * The Companion's address for a phone that is already paired: it opens the
 * page and logs in with its passkey, so it never needs a second pairing.
 */
export function CompanionPhoneLinkDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation("companion")
  const [link, setLink] = useState<{ url: string; qr: string } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    let active = true
    let qr: string | null = null
    void companionPhoneLink().then(next => {
      if (!active) return
      qr = URL.createObjectURL(new Blob([next.qrSvg], { type: "image/svg+xml" }))
      setLink({ url: next.url, qr })
    }).catch(cause => { if (active) setError(String(cause)) })
    return () => {
      active = false
      if (qr) URL.revokeObjectURL(qr)
    }
  }, [])

  const alert = error && <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(error, t)}</p>
  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent data-testid="companion-phone-link-dialog" className="sm:max-w-[380px]">
        <DialogHeader>
          <DialogTitle>{t("phoneLink.title")}</DialogTitle>
          <DialogDescription>{t("phoneLink.description")}</DialogDescription>
        </DialogHeader>
        {link ? <div className="flex flex-col items-center gap-2">
          <img src={link.qr} alt={t("phoneLink.qrAlt")} className="size-[240px] rounded-md bg-white p-2" />
          <p className="break-all text-center font-mono text-[12px]">{link.url}</p>
          <Button size="sm" variant="outline" onClick={() => void writeText(link.url).then(() => setCopied(true), cause => setError(String(cause)))}>
            {t(copied ? "phoneLink.copied" : "phoneLink.copy")}
          </Button>
          <p className="text-[12px] text-muted-foreground">{t("phoneLink.homeScreen")}</p>
          {alert}
        </div>
          : alert || <p className="text-[12px] text-muted-foreground">{t("phoneLink.loading")}</p>}
      </DialogContent>
    </Dialog>
  )
}
