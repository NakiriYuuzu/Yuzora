interface SplitRatioIndicatorProps {
    text: string
}

export function SplitRatioIndicator({ text }: SplitRatioIndicatorProps) {
    return (
        <span
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-1/2 z-20 -translate-x-1/2 -translate-y-1/2 whitespace-nowrap rounded-[6px] border border-(--line-1) bg-(--paper-0) px-[8px] py-[4px] text-[11px] font-medium text-(--ink-1) shadow-(--shadow-sm)"
        >
            {text}
        </span>
    )
}
