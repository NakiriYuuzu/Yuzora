import { Plus } from "lucide-react"

interface DashedActionButtonProps {
  label: string
  onClick: () => void
}

/**
 * Dashed "New X" action button — design reference §4 (dashed add button):
 * 1px dashed --line-2, hover switches to an accent-tinted border/text/bg
 * (matches StatusBar's rgba(var(--yz-accent-rgb),0.14) hover tint). Shared
 * by the Database/SSH/Agent nav panels; all call sites are no-ops since
 * creation flows land in a later task.
 */
export function DashedActionButton({ label, onClick }: DashedActionButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex h-[34px] w-full shrink-0 items-center justify-center gap-[6px] rounded-[10px] border border-dashed border-(--line-2) text-[12.5px] font-medium text-(--ink-3) transition-colors hover:border-(--yz-accent)/60 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] hover:text-(--yz-accent-ink)"
    >
      <Plus className="size-[14px]" aria-hidden="true" />
      {label}
    </button>
  )
}
