import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { KeyRound, QrCode } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Message, Page, Progress } from "../components/Page"
import { useUnmountSignal } from "../components/useUnmountSignal"
import { api, ApiError } from "../lib/api"
import { defaultLabel, labelProblem } from "../lib/label"
import { claimPairing, pollPairing, SasMismatchError, type PairingLink } from "../lib/pairing"
import { createPasskey, passkeyError, passkeysSupported } from "../lib/webauthn"
import { navigate, storage } from "../store"

type Failure = "invalid-link" | "rejected" | "expired" | "voided" | "sas-mismatch" | "claim-failed" | "register-failed" | "policy-expired"
type Step =
  | { name: "label" }
  | { name: "claiming" }
  | { name: "waiting"; sas: string }
  | { name: "preparing"; sas: string }
  | { name: "ready"; sas: string; options: unknown; error?: string }
  | { name: "registering"; sas: string }
  | { name: "failed"; failure: Failure }

export function PairScreen({ link }: { link: PairingLink | "invalid" | null }) {
  const { t } = useTranslation()
  if (!link) {
    return (
      <Page title={t("pair.title")}>
        <div className="flex flex-col items-center gap-4 px-6 py-10 text-center">
          <QrCode className="size-12 text-muted-foreground" aria-hidden />
          <p className="text-base">{t("pair.noLink")}</p>
          {/* Always offered: a home-screen app on iOS starts with its own empty storage. */}
          <Button variant="outline" size="lg" className="h-11" onClick={() => navigate({ name: "login" })}>{t("pair.toLogin")}</Button>
        </div>
      </Page>
    )
  }
  return <Pairing link={link} />
}

function Pairing({ link }: { link: PairingLink | "invalid" }) {
  const { t } = useTranslation()
  const [label, setLabel] = useState(() => storage.label() ?? defaultLabel(navigator.userAgent))
  const [step, setStep] = useState<Step>(link === "invalid" ? { name: "failed", failure: "invalid-link" } : { name: "label" })
  // Each of these may run once per link: a second claim voids the pairing and
  // the options request consumes the registration ticket.
  const claimed = useRef(false)
  const prepared = useRef(false)
  const signal = useUnmountSignal()
  const pairingId = link === "invalid" ? "" : link.pairingId
  const problem = labelProblem(label)

  const claim = async () => {
    if (link === "invalid" || claimed.current || problem) return
    claimed.current = true
    setStep({ name: "claiming" })
    storage.setLabel(label)
    let claimToken: string
    let sas: string
    try {
      ({ claimToken, sas } = await claimPairing(link, label))
    } catch (error) {
      setStep({ name: "failed", failure: error instanceof SasMismatchError ? "sas-mismatch" : error instanceof ApiError && error.policyExpired ? "policy-expired" : "claim-failed" })
      return
    }
    setStep({ name: "waiting", sas })
    let result: Awaited<ReturnType<typeof pollPairing>>
    try {
      result = await pollPairing(link.pairingId, claimToken, signal())
    } catch {
      return
    }
    if (result.state !== "approved") {
      setStep({ name: "failed", failure: result.state === "rejected" ? "rejected" : result.state === "expired" ? "expired" : "voided" })
      return
    }
    if (prepared.current) return
    prepared.current = true
    setStep({ name: "preparing", sas })
    try {
      // Fetched now, so the tap below can start the ceremony directly.
      const options = await api.registrationOptions({ pairingId: link.pairingId, registrationTicket: result.ticket })
      setStep({ name: "ready", sas, options })
    } catch {
      setStep({ name: "failed", failure: "register-failed" })
    }
  }

  const register = async (sas: string, options: unknown) => {
    setStep({ name: "registering", sas })
    let credential: unknown
    try {
      credential = await createPasskey(options)
    } catch (error) {
      // Nothing reached the gateway; the same options can be tried again.
      setStep({ name: "ready", sas, options, error: `passkey.${passkeyError(error)}` })
      return
    }
    try {
      await api.registrationFinish({ pairingId, credential })
    } catch {
      setStep({ name: "failed", failure: "register-failed" })
      return
    }
    storage.setPaired()
    navigate({ name: "login", reason: "registered" })
  }

  return (
    <Page title={t("pair.title")}>
      <div className="flex flex-col gap-5 px-5 py-6">
        {step.name === "label" && (
          <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); void claim() }}>
            <label htmlFor="yzc-label" className="text-base font-medium">{t("pair.labelTitle")}</label>
            <Input id="yzc-label" value={label} maxLength={80} autoComplete="off" onChange={(event) => setLabel(event.target.value)} aria-invalid={problem ? true : undefined} className="h-11 text-base" />
            <p className={problem ? "text-sm text-destructive" : "text-sm text-muted-foreground"}>{problem ? t(`pair.labelProblem.${problem}`) : t("pair.labelHint")}</p>
            {!passkeysSupported() && <Message tone="warning">{t("passkey.unsupported")}</Message>}
            <Button type="submit" size="lg" className="h-11" disabled={!!problem}>{t("pair.claim")}</Button>
          </form>
        )}
        {step.name === "claiming" && <Progress text={t("pair.claiming")} />}
        {"sas" in step && (
          <section aria-labelledby="yzc-sas" className="flex flex-col items-center gap-3 rounded-xl border bg-card px-4 py-6 text-center">
            <h2 id="yzc-sas" className="text-base font-medium">{t("pair.sasTitle")}</h2>
            <p className="font-mono text-4xl font-semibold tracking-[0.3em] tabular-nums" aria-label={step.sas.split("").join(" ")}>{step.sas}</p>
            <p className="text-sm text-muted-foreground">{step.name === "waiting" ? t("pair.sasHint") : t("pair.approvedHint")}</p>
          </section>
        )}
        {step.name === "waiting" && <Progress text={t("pair.waiting")} />}
        {step.name === "preparing" && <Progress text={t("pair.preparing")} />}
        {step.name === "registering" && <Progress text={t("pair.registering")} />}
        {step.name === "ready" && (
          <div className="flex flex-col gap-3">
            {step.error && <Message tone="warning">{t(step.error)}</Message>}
            <Button size="lg" className="h-11" onClick={() => void register(step.sas, step.options)}>
              <KeyRound aria-hidden />
              {t("pair.createPasskey")}
            </Button>
          </div>
        )}
        {step.name === "failed" && (
          <div className="flex flex-col gap-3">
            <Message tone="error">{t(`pair.failure.${step.failure}`)}</Message>
            {step.failure !== "policy-expired" && <p className="text-sm text-muted-foreground">{t("pair.rescan")}</p>}
          </div>
        )}
      </div>
    </Page>
  )
}
