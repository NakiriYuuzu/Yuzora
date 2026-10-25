import { useTranslation } from "react-i18next"
import { ArrowLeft } from "lucide-react"
import { Button } from "@/components/ui/button"
import { goBack } from "../store"

/** The in-app twin of the system back gesture. */
export function BackButton() {
  const { t } = useTranslation()
  return (
    <Button variant="ghost" size="icon" className="size-11" aria-label={t("common.back")} onClick={goBack}>
      <ArrowLeft aria-hidden />
    </Button>
  )
}
