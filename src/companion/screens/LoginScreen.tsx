import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { KeyRound } from "lucide-react"
import { Button } from "@yuzora/shared/components/ui/button"
import { abortLogout, connect } from "../client"
import { Message, Page } from "../components/Page"
import { api } from "../lib/api"
import { failureKey } from "../lib/errors"
import { getPasskey } from "../lib/webauthn"
import { navigate, storage } from "../store"

// Login challenges live 120 s; fetch a fresh one before that.
const OPTIONS_REFRESH = 100_000

export function LoginScreen({ reason }: { reason?: "expired" | "registered" | "logged-out" }) {
  const { t } = useTranslation()
  const [options, setOptions] = useState<unknown>(null)
  const [generation, setGeneration] = useState(0)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Fetched ahead of the tap: Safari only starts a ceremony straight from a gesture.
  useEffect(() => {
    let live = true
    api.loginOptions().then(
      (value) => live && setOptions(value),
      (failure: unknown) => live && setError(failureKey(failure)),
    )
    const timer = setTimeout(() => setGeneration((value) => value + 1), OPTIONS_REFRESH)
    return () => {
      live = false
      clearTimeout(timer)
    }
  }, [generation])

  const signIn = async () => {
    if (!options || busy) return
    const current = options
    setOptions(null)
    setBusy(true)
    setError(null)
    abortLogout()
    try {
      const login = await api.loginFinish(await getPasskey(current))
      storage.setBinding(login.binding)
      storage.setPaired()
      navigate({ name: "home" })
      connect()
    } catch (failure) {
      setError(failureKey(failure, "login.rejected"))
      setGeneration((value) => value + 1)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Page title={t("login.title")}>
      <div className="flex flex-col gap-4 px-5 py-8">
        {reason && <Message>{t(`login.reason.${reason}`)}</Message>}
        <p className="text-base">{t("login.hint")}</p>
        {error && <Message tone="error">{t(error)}</Message>}
        <Button size="lg" className="h-11" disabled={!options || busy} onClick={() => void signIn()}>
          <KeyRound aria-hidden />
          {t("login.button")}
        </Button>
        <Button variant="ghost" className="h-11" onClick={() => navigate({ name: "pair", link: null })}>{t("login.pairAgain")}</Button>
      </div>
    </Page>
  )
}
