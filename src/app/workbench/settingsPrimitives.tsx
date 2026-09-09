import { Lock } from "lucide-react"
import { useId } from "react"

import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { Field,FieldContent,FieldDescription,FieldError,FieldLabel,FieldLegend,FieldSet } from "@/components/ui/field"
import { ToggleGroup,ToggleGroupItem } from "@/components/ui/toggle-group"

/** A labelled, flat settings section; the public helper name is retained. */
export function SettingCard({
  label,
  sub,
  children,
}: {
  label: string
  sub?: string
  children: React.ReactNode
}) {
  return (
    <FieldSet className="settings-section-block" data-settings-label={label}>
      <FieldLegend>{label}</FieldLegend>
      {sub && <FieldDescription>{sub}</FieldDescription>}
      <div className="settings-section-controls">{children}</div>
    </FieldSet>
  )
}

/** Design reference segmented control: sunken --paper-2 track, --yz-solid thumb. */
export function Segmented({
  label,
  options,
  value,
  onChange,
}: {
  label: string
  options: { id: string; label: string }[]
  value: string
  onChange: (id: string) => void
}) {
  return (
    <ToggleGroup
      type="single"
      role="radiogroup"
      aria-label={label}
      value={value}
      onValueChange={id=>{if(id)onChange(id)}}
      className="settings-segmented"
    >
      {options.map((option) => {
        return (
          <ToggleGroupItem
            key={option.id}
            value={option.id}
            role="radio"
            aria-checked={option.id === value}
            type="button"
          >
            {option.label}
          </ToggleGroupItem>
        )
      })}
    </ToggleGroup>
  )
}

/** Design reference toggle row: label + sub on the left, switch on the right. */
export function ToggleRow({
  label,
  sub,
  locked,
  disabled,
  checked,
  onCheckedChange,
}: {
  label: string
  sub: string
  locked?: boolean
  disabled?: boolean
  checked: boolean
  onCheckedChange: (checked: boolean) => void
}) {
  const id=useId()
  return (
    <Field orientation="horizontal" className="settings-toggle-row" data-settings-label={label}>
      <FieldContent>
        <FieldLabel htmlFor={id}>
          {label}
          {locked && <Lock className="size-[11px] shrink-0 text-[#c2293f]" aria-hidden="true" />}
        </FieldLabel>
        <FieldDescription id={`${id}-hint`}>{sub}</FieldDescription>
      </FieldContent>
      <Switch
        id={id}
        aria-describedby={`${id}-hint`}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        aria-label={label}
        className="yz-switch"
      />
    </Field>
  )
}

export function SettingsTextInput({
  label,
  value,
  onChange,
  placeholder,
  disabled = false,
  type = "text",
  error,
  errorId,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  disabled?: boolean
  type?: "text" | "number"
  error?: string | null
  errorId?: string
}) {
  const id=useId()
  return (
    <Field data-invalid={!!error} data-disabled={disabled} className="settings-text-field">
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      <Input
        id={id}
        aria-label={label}
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? errorId : undefined}
        onChange={(event) => onChange(event.currentTarget.value)}
        className="font-mono"
      />
      {error ? (
        <FieldError id={errorId} role="alert">
          {error}
        </FieldError>
      ) : null}
    </Field>
  )
}
