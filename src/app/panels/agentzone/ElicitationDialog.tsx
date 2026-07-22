import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { ElicitationField, ElicitationResponsePayload } from "@/agent/acpConnection"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { useAgentStore, type PendingElicitation } from "@/state/agentStore"

// ACP form elicitation 的 modal（spec P3／P4）：渲染 pendingElicitations 佇列頭。
// 兩個特化捷徑（單一 string enum → 點選即送出；單一 boolean → 是/否雙鈕），
// 其餘走通用表單（string/number/integer/boolean/array multiselect 欄位＋
// required 驗證）。關閉＝cancel。
export function ElicitationDialog({ sessionId }: { sessionId: string }) {
  const head = useAgentStore((state) => state.pendingElicitations.get(sessionId)?.[0])
  const respondElicitation = useAgentStore((state) => state.respondElicitation)
  if (!head) return null
  return (
    <ElicitationForm
      key={head.id}
      pending={head}
      onRespond={(response) => respondElicitation(sessionId, head.id, response)}
    />
  )
}

type FieldValue = string | number | boolean | string[]

function initialValues(fields: ElicitationField[]): Record<string, FieldValue> {
  const values: Record<string, FieldValue> = {}
  for (const field of fields) {
    if (field.defaultValue !== undefined) values[field.key] = field.defaultValue
    else if (field.type === "boolean") values[field.key] = false
  }
  return values
}

function ElicitationForm({
  pending,
  onRespond,
}: {
  pending: PendingElicitation
  onRespond: (response: ElicitationResponsePayload) => void
}) {
  const { t } = useTranslation("panels")
  const { request } = pending
  const [values, setValues] = useState<Record<string, FieldValue>>(
    () => initialValues(request.fields)
  )
  const [open, setOpen] = useState(true)
  // 單次 respond 守衛：accept 走 setOpen(false) 收合動畫時，close effect 不得再補 cancel。
  const respondedRef = useRef(false)
  const respondOnce = (response: ElicitationResponsePayload) => {
    if (respondedRef.current) return
    respondedRef.current = true
    onRespond(response)
  }

  useEffect(() => {
    if (!open) respondOnce({ action: "cancel" })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- respondOnce 每 render 重建但 ref 守衛使其冪等
  }, [open])

  // array（multiselect）不能點選即送出——排除在 soloEnum 捷徑外。
  const soloEnum =
    request.fields.length === 1 && request.fields[0].options && request.fields[0].type !== "array"
      ? request.fields[0]
      : null
  const soloBoolean =
    request.fields.length === 1 && request.fields[0].type === "boolean" ? request.fields[0] : null

  const missingRequired = useMemo(
    () =>
      request.fields.some((field) => {
        if (!field.required) return false
        const value = values[field.key]
        if (field.type === "boolean") return value === undefined
        if (field.type === "array") return !Array.isArray(value) || value.length === 0
        return value === undefined || value === ""
      }),
    [request.fields, values]
  )

  const accept = (content: Record<string, FieldValue>) => {
    respondOnce({ action: "accept", content })
    setOpen(false)
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        data-testid="agent-elicitation-dialog"
        className="max-w-md border-(--line-1) bg-(--yz-glass) backdrop-blur-xl"
      >
        <DialogHeader>
          <DialogTitle>{request.title || t("agentZonePanel.elicitTitle")}</DialogTitle>
          {request.message && request.message !== request.title && (
            <DialogDescription>{request.message}</DialogDescription>
          )}
        </DialogHeader>

        {soloEnum ? (
          <div className="flex flex-col gap-1.5" role="listbox" aria-label={soloEnum.title ?? request.message}>
            {soloEnum.options!.map((option) => (
              <Button
                key={option.value}
                type="button"
                role="option"
                aria-selected={false}
                variant="outline"
                className="h-auto min-h-8 justify-start whitespace-normal text-left"
                onClick={() => accept({ [soloEnum.key]: option.value })}
              >
                <span className="flex flex-col items-start gap-0.5">
                  <span>{option.label}</span>
                  {option.description && (
                    <span className="text-[11px] font-normal text-(--ink-3)">{option.description}</span>
                  )}
                </span>
              </Button>
            ))}
          </div>
        ) : soloBoolean ? null : (
          <div className="flex flex-col gap-3">
            {request.fields.map((field) => (
              <FieldRow
                key={field.key}
                field={field}
                value={values[field.key]}
                onChange={(value) => setValues((current) => ({ ...current, [field.key]: value }))}
              />
            ))}
          </div>
        )}

        <DialogFooter>
          {soloBoolean ? (
            <>
              <Button type="button" variant="outline" onClick={() => accept({ [soloBoolean.key]: false })}>
                {t("agentZonePanel.elicitNo")}
              </Button>
              <Button type="button" onClick={() => accept({ [soloBoolean.key]: true })}>
                {t("agentZonePanel.elicitYes")}
              </Button>
            </>
          ) : (
            <>
              <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
                {t("agentZonePanel.elicitCancel")}
              </Button>
              {!soloEnum && (
                <Button
                  type="button"
                  disabled={missingRequired}
                  onClick={() => accept({ ...values })}
                >
                  {t("agentZonePanel.elicitSubmit")}
                </Button>
              )}
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function FieldRow({
  field,
  value,
  onChange,
}: {
  field: ElicitationField
  value: FieldValue | undefined
  onChange: (value: FieldValue) => void
}) {
  const label = field.title ?? field.key
  const labelNode = (
    <span className="text-[11.5px] font-semibold text-(--ink-1)">
      {label}
      {field.required && <span aria-hidden="true" className="text-destructive"> *</span>}
    </span>
  )
  if (field.type === "boolean") {
    return (
      <label className="flex items-center justify-between gap-3">
        <span className="flex flex-col">
          {labelNode}
          {field.description && <span className="text-[11px] text-(--ink-3)">{field.description}</span>}
        </span>
        <Switch checked={value === true} onCheckedChange={(checked) => onChange(checked)} aria-label={label} />
      </label>
    )
  }
  if (field.options) {
    const multiselect = field.type === "array"
    const selectedValues = multiselect && Array.isArray(value) ? value : []
    const isSelected = (candidate: string) =>
      multiselect ? selectedValues.includes(candidate) : value === candidate
    const toggle = (candidate: string) => {
      if (!multiselect) {
        onChange(candidate)
        return
      }
      onChange(
        selectedValues.includes(candidate)
          ? selectedValues.filter((entry) => entry !== candidate)
          : [...selectedValues, candidate]
      )
    }
    return (
      <div className="flex flex-col gap-1.5">
        {labelNode}
        <div
          className="flex flex-wrap gap-1.5"
          role="listbox"
          aria-label={label}
          aria-multiselectable={multiselect || undefined}
        >
          {field.options.map((option) => (
            <Button
              key={option.value}
              type="button"
              role="option"
              aria-selected={isSelected(option.value)}
              size="sm"
              variant={isSelected(option.value) ? "default" : "outline"}
              onClick={() => toggle(option.value)}
            >
              {option.label}
            </Button>
          ))}
        </div>
      </div>
    )
  }
  if (field.type === "number" || field.type === "integer") {
    return (
      <label className="flex flex-col gap-1.5">
        {labelNode}
        <Input
          type="number"
          value={value === undefined ? "" : String(value)}
          step={field.type === "integer" ? 1 : undefined}
          onChange={(event) => {
            const parsed = field.type === "integer"
              ? Number.parseInt(event.target.value, 10)
              : Number.parseFloat(event.target.value)
            if (!Number.isNaN(parsed)) onChange(parsed)
          }}
        />
      </label>
    )
  }
  return (
    <label className="flex flex-col gap-1.5">
      {labelNode}
      {field.multiline ? (
        <Textarea
          value={typeof value === "string" ? value : ""}
          placeholder={field.description}
          rows={5}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : (
        <Input
          value={typeof value === "string" ? value : ""}
          placeholder={field.description}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
    </label>
  )
}
