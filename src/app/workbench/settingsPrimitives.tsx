import { Lock } from "lucide-react"

import { cn } from "@/lib/utils"
import { Switch } from "@/components/ui/switch"

/** Design reference settings card: --yz-panel surface, 13px radius. */
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
    <div className="rounded-[13px] border border-(--line-1) bg-(--yz-panel) p-[14px]">
      <div className={cn("text-[12.5px] font-medium text-(--ink-1)", !sub && "mb-[9px]")}>
        {label}
      </div>
      {sub && <div className="mt-[2px] mb-[9px] text-[11px] text-(--ink-3)">{sub}</div>}
      {children}
    </div>
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
    <div
      role="radiogroup"
      aria-label={label}
      className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
    >
      {options.map((option) => {
        const active = option.id === value
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(option.id)}
            className={cn(
              "flex h-[28px] flex-1 items-center justify-center rounded-[8px] text-[11.5px] transition-all duration-[140ms] ease-(--ease-out)",
              active
                ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                : "font-medium text-(--ink-3) hover:text-(--ink-1)"
            )}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}

/** Design reference toggle row: label + sub on the left, switch on the right. */
export function ToggleRow({
  label,
  sub,
  locked,
  checked,
  onCheckedChange,
}: {
  label: string
  sub: string
  locked?: boolean
  checked: boolean
  onCheckedChange: (checked: boolean) => void
}) {
  return (
    <div className="flex items-center gap-[12px] border-b border-(--line-1) px-[6px] py-[13px]">
      <div className="flex min-w-0 flex-1 flex-col gap-[2px]">
        <span className="flex items-center gap-[6px] text-[13px] font-medium text-(--ink-1)">
          {label}
          {locked && <Lock className="size-[11px] shrink-0 text-[#c2293f]" aria-hidden="true" />}
        </span>
        <span className="text-[11px] leading-[1.45] text-(--ink-3)">{sub}</span>
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onCheckedChange}
        aria-label={label}
        className="yz-switch"
      />
    </div>
  )
}

export function SettingsTextInput({
  label,
  value,
  onChange,
  placeholder,
  disabled = false,
  type = "text",
}: {
  label: string
  value: string
  onChange: (value: string) => void
  placeholder?: string
  disabled?: boolean
  type?: "text" | "number"
}) {
  return (
    <label className="flex flex-col gap-[6px]">
      <span className="text-[11.5px] font-medium text-(--ink-2)">{label}</span>
      <input
        aria-label={label}
        type={type}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(event) => onChange(event.currentTarget.value)}
        className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] font-mono text-[11.5px] text-(--ink-1) outline-none transition-colors placeholder:text-(--ink-4) focus:border-(--yz-accent) disabled:cursor-not-allowed disabled:opacity-60"
      />
    </label>
  )
}
