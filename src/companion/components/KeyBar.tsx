import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { KEY_BAR } from "../lib/keys"
import type { AgentKey } from "../lib/protocol"

export function KeyBar({ disabled, onKey }: { disabled: boolean; onKey: (key: AgentKey) => void }) {
  const { t } = useTranslation()
  return (
    <div role="toolbar" aria-label={t("control.keys")} className="flex gap-1.5 overflow-x-auto px-3 pb-1">
      {KEY_BAR.map(({ key, label }) => (
        <Button key={key} variant="outline" className="h-11 min-w-11 shrink-0 px-3 font-mono" disabled={disabled} aria-label={t(`control.key.${key}`)} onClick={() => onKey(key)}>
          {label}
        </Button>
      ))}
    </div>
  )
}
