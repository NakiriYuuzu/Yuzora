import { useEffect } from "react"
import { I18nextProvider, useTranslation } from "react-i18next"
import { X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { noteActivity, refresh, retryNow } from "./client"
import { i18n } from "./i18n"
import { HomeScreen } from "./screens/HomeScreen"
import { LoginScreen } from "./screens/LoginScreen"
import { PairScreen } from "./screens/PairScreen"
import { PaneScreen } from "./screens/PaneScreen"
import { applyUpdate, useUpdate } from "./serviceWorker"
import { notify, useCompanion } from "./store"

export function App() {
  const screen = useCompanion((state) => state.screen)

  useEffect(() => {
    // Back in the foreground: reconnect now and catch up on what events missed.
    const visible = () => {
      if (document.visibilityState !== "visible") return
      retryNow()
      if (useCompanion.getState().socket === "ready") void refresh()
    }
    document.addEventListener("visibilitychange", visible)
    document.addEventListener("pointerdown", noteActivity, { passive: true })
    document.addEventListener("keydown", noteActivity)
    return () => {
      document.removeEventListener("visibilitychange", visible)
      document.removeEventListener("pointerdown", noteActivity)
      document.removeEventListener("keydown", noteActivity)
    }
  }, [])

  return (
    <I18nextProvider i18n={i18n}>
      <div className="mx-auto w-full max-w-3xl">
        {screen.name === "pair" && <PairScreen link={screen.link} />}
        {screen.name === "login" && <LoginScreen reason={screen.reason} />}
        {screen.name === "home" && <HomeScreen />}
        {screen.name === "pane" && <PaneScreen key={`${screen.session}\0${screen.pane}`} session={screen.session} pane={screen.pane} />}
      </div>
      <Notices />
    </I18nextProvider>
  )
}

function Notices() {
  const { t } = useTranslation()
  const notice = useCompanion((state) => state.notice)
  const update = useUpdate((state) => state.waiting)
  // Confirmations fade; warnings and errors stay until dismissed.
  useEffect(() => {
    if (notice?.tone !== "info") return
    const timer = setTimeout(() => notify(null), 2500)
    return () => clearTimeout(timer)
  }, [notice])
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-0 z-20 flex flex-col gap-2 p-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]">
      {notice && (
        <div role={notice.tone === "info" ? "status" : "alert"} className="pointer-events-auto mx-auto flex w-full max-w-xl items-start gap-2 rounded-xl border bg-popover px-3 py-2.5 text-sm shadow-lg">
          <p className={`flex-1 ${notice.tone === "error" ? "text-destructive" : ""}`}>{t(notice.key, notice.values)}</p>
          <Button variant="ghost" size="icon" className="-my-1 size-9" aria-label={t("common.dismiss")} onClick={() => notify(null)}>
            <X aria-hidden />
          </Button>
        </div>
      )}
      {update && (
        <div className="pointer-events-auto mx-auto flex w-full max-w-xl items-center gap-2 rounded-xl border bg-popover px-3 py-2 text-sm shadow-lg">
          <p className="flex-1">{t("update.ready")}</p>
          <Button className="h-10" onClick={applyUpdate}>{t("update.reload")}</Button>
        </div>
      )}
    </div>
  )
}
