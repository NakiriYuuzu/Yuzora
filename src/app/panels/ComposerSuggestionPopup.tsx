import { useEffect, useRef, type CSSProperties, type ReactNode } from "react"

import { composerSuggestionOptionId } from "@/app/panels/agentComposerSuggestions"

export type ComposerSuggestionPopupStatus = "ready" | "loading" | "error"

export interface ComposerSuggestionItem<T> {
  key: string
  value: T
  ariaLabel: string
  label: ReactNode
  description?: ReactNode
  leading?: ReactNode
}

export interface ComposerSuggestionPopupProps<T> {
  id: string
  ariaLabel: string
  items: readonly ComposerSuggestionItem<T>[]
  selectedIndex: number
  onSelect: (value: T) => void
  onAfterSelect?: () => void
  header?: ReactNode
  status?: ComposerSuggestionPopupStatus
  loadingSlot?: ReactNode
  emptySlot?: ReactNode
  errorSlot?: ReactNode
  style?: CSSProperties
}

export function ComposerSuggestionPopup<T>({
  id,
  ariaLabel,
  items,
  selectedIndex,
  onSelect,
  onAfterSelect,
  header,
  status = "ready",
  loadingSlot,
  emptySlot,
  errorSlot,
  style,
}: ComposerSuggestionPopupProps<T>) {
  const activeOptionRef = useRef<HTMLButtonElement>(null)
  const activeIndex = status === "ready" && items.length > 0
    ? Math.max(0, Math.min(selectedIndex, items.length - 1))
    : null
  const activeKey = activeIndex === null ? null : items[activeIndex]?.key ?? null

  useEffect(() => {
    if (activeKey === null) return
    activeOptionRef.current?.scrollIntoView?.({ block: "nearest" })
  }, [activeKey])

  let statusContent: ReactNode
  if (status === "loading") statusContent = loadingSlot
  else if (status === "error") statusContent = errorSlot
  else statusContent = emptySlot
  const showItems = status === "ready" && items.length > 0

  return (
    <div
      id={id}
      className="yzs"
      role="listbox"
      aria-label={ariaLabel}
      aria-busy={status === "loading" || undefined}
      style={{
        position: "absolute",
        left: 14,
        right: 14,
        bottom: 60,
        zIndex: 20,
        maxHeight: 300,
        overflowY: "auto",
        background: "var(--frost-light)",
        backdropFilter: "var(--blur-frost)",
        WebkitBackdropFilter: "var(--blur-frost)",
        border: "1px solid var(--line-2)",
        borderRadius: 14,
        boxShadow: "var(--shadow-xl)",
        padding: 7,
        animation: "yzpop 130ms var(--ease-spring)",
        ...style,
      }}
    >
      {header !== undefined && (
        <div
          style={{
            font: "var(--text-label)",
            fontSize: 9.5,
            letterSpacing: "0.09em",
            color: "var(--ink-3)",
            textTransform: "uppercase",
            padding: "8px 10px 5px",
          }}
        >
          {header}
        </div>
      )}

      {!showItems ? (
        <div className="px-[11px] py-[10px] text-[12px] text-(--ink-3)">
          {statusContent}
        </div>
      ) : (
        items.map((item, index) => {
          const selected = index === activeIndex
          return (
            <button
              key={item.key}
              ref={selected ? activeOptionRef : undefined}
              id={composerSuggestionOptionId(id, item.key)}
              type="button"
              role="option"
              aria-label={item.ariaLabel}
              aria-selected={selected}
              onPointerDown={(event) => event.preventDefault()}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                onSelect(item.value)
                onAfterSelect?.()
              }}
              style={{
                width: "100%",
                display: "flex",
                alignItems: "center",
                gap: 11,
                height: 38,
                padding: "0 11px",
                borderRadius: 10,
                cursor: "pointer",
                transition: "background 110ms",
                background: selected ? "var(--yz-active)" : "transparent",
                boxShadow: selected ? "inset 0 0 0 1px var(--line-1)" : undefined,
                border: 0,
                textAlign: "left",
              }}
            >
              {item.leading !== undefined && (
                <span
                  aria-hidden="true"
                  style={{
                    width: 26,
                    height: 26,
                    flex: "0 0 auto",
                    borderRadius: 8,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    background: "var(--mint-soft)",
                    color: "#0f7a55",
                  }}
                >
                  {item.leading}
                </span>
              )}
              <span
                style={{
                  fontFamily: "var(--font-mono)",
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: "var(--ink-0)",
                  flex: "0 0 auto",
                }}
              >
                {item.label}
              </span>
              {item.description !== undefined && (
                <span
                  style={{
                    flex: 1,
                    minWidth: 0,
                    fontSize: 12,
                    color: "var(--ink-3)",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {item.description}
                </span>
              )}
            </button>
          )
        })
      )}
    </div>
  )
}
