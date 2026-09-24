import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"
import { Card } from "@/components/ui/card"
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item"
import { Kbd } from "@/components/ui/kbd"
import { ScrollArea } from "@/components/ui/scroll-area"
import { AgentLogo } from "./AgentLogo"

export interface AgentSwitcherItem {
  key: string
  title: string
  subtitle: string
  status?: string
  statusLabel?: string
  logoKind: string | null
  logoLabel: string
}

/**
 * Alt+Tab-style Agent switcher. Custom composition over shadcn Card/Item/Kbd:
 * shadcn has no hold-to-switch primitive. It is not a modal and never takes
 * focus, so the terminal keeps receiving the held modifier's key events.
 */
export function AgentSwitcher({ items, index, holdLabel, onCommit, onHighlight }: {
  items: readonly AgentSwitcherItem[]
  index: number
  holdLabel: string
  onCommit: (index: number) => void
  onHighlight: (index: number) => void
}) {
  const { t } = useTranslation("spaceTree")
  const current = items[index]
  return createPortal(
    <div className="agent-switcher-layer">
      <Card className="agent-switcher" data-testid="agent-switcher">
        <ScrollArea className="agent-switcher-scroll">
          <div role="listbox" aria-label={t("agentSwitcher")} aria-activedescendant={current ? `agent-switcher-${index}` : undefined} className="agent-switcher-list">
            {items.map((item, position) => (
              <Item
                key={item.key}
                id={`agent-switcher-${position}`}
                role="option"
                aria-selected={position === index}
                size="sm"
                className="agent-switcher-item"
                data-highlighted={position === index || undefined}
                // Keep focus where it is (usually the terminal).
                onMouseDown={(event) => event.preventDefault()}
                onMouseEnter={() => onHighlight(position)}
                onClick={() => onCommit(position)}
              >
                <ItemMedia className="tree-agent-avatar">
                  <AgentLogo kind={item.logoKind} label={item.logoLabel} />
                  {item.status && <span className="tree-status-dot" data-status={item.status} />}
                </ItemMedia>
                <ItemContent>
                  <ItemTitle>{item.title}</ItemTitle>
                  <ItemDescription>{item.subtitle}</ItemDescription>
                </ItemContent>
                {item.statusLabel && (
                  <ItemActions>
                    <span className="tree-agent-status" data-status={item.status}>{item.statusLabel}</span>
                  </ItemActions>
                )}
              </Item>
            ))}
          </div>
        </ScrollArea>
        <p className="agent-switcher-hint">
          <Kbd>{holdLabel}</Kbd> {t("agentSwitcherRelease")} · <Kbd>Esc</Kbd> {t("agentSwitcherCancel")}
        </p>
      </Card>
      <span className="sr-only" aria-live="polite">{current?.title ?? ""}</span>
    </div>,
    document.body,
  )
}
