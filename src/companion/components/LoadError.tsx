import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { rpcErrorKey } from "../lib/errors"
import { Message } from "./Page"

/** A failed read: what happened, and a way to try again (e.g. after trusting the workspace on the desktop). */
export function LoadError({ code, onRetry }: { code: string; onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col gap-2">
      <Message tone="warning">{t(rpcErrorKey(code))}</Message>
      <Button variant="outline" className="h-11" onClick={onRetry}>{t("git.retry")}</Button>
    </div>
  )
}
