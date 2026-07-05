import { useEffect, useRef } from "react"

import { CONTEXT_MENU_DEFS } from "@/app/workbench/contextMenuDefs"
import { cn } from "@/lib/utils"
import { runContextMenuAction, useContextMenuStore } from "@/state/contextMenuStore"

const MENU_WIDTH = 212
const EDGE_GAP = 8
// 邊緣 clamp 用的平面估算（item 31px + 間距；設計文件用同一個常數估）。
const ENTRY_HEIGHT = 33
const MENU_PADDING = 10

// Pointer 座標是 visual px；選單渲染在 AppShell 縮放過的 <body>（body{zoom}）
// 裡，1 layout px = zoom 個 visual px，所以這裡統一除以 zoom 後再對 layout
// viewport 做右/下緣 clamp。
export function placeMenu(
  x: number,
  y: number,
  entryCount: number,
  zoom: number,
  viewportWidth: number,
  viewportHeight: number
) {
  const vw = viewportWidth / zoom
  const vh = viewportHeight / zoom
  const menuHeight = entryCount * ENTRY_HEIGHT + MENU_PADDING
  let left = x / zoom
  let top = y / zoom
  if (left + MENU_WIDTH > vw - EDGE_GAP) left = vw - MENU_WIDTH - EDGE_GAP
  if (top + menuHeight > vh - EDGE_GAP) top = Math.max(EDGE_GAP, vh - menuHeight - EDGE_GAP)
  return { left, top }
}

function bodyZoom() {
  const zoom = Number.parseFloat(document.body.style.getPropertyValue("zoom"))
  return Number.isFinite(zoom) && zoom > 0 ? zoom : 1
}

/**
 * 全 app 唯一的右鍵選單 overlay — 設計文件 "CONTEXT MENU (all regions)"
 * （L1467）。內容由 CONTEXT_MENU_DEFS 依 store 的 kind 決定；UI-only 階段
 * 每個項目點擊只走 runContextMenuAction（關閉 + log）。
 */
export function ContextMenu() {
  const kind = useContextMenuStore((s) => s.kind)
  const x = useContextMenuStore((s) => s.x)
  const y = useContextMenuStore((s) => s.y)
  const payload = useContextMenuStore((s) => s.payload)
  const close = useContextMenuStore((s) => s.close)
  const menuRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!kind) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close()
    }
    // pointerdown（而非 click）關閉：在其他區域按下右鍵時先關掉舊選單，
    // 緊接著的 contextmenu 事件會直接 open 新選單 — 一次右鍵完成切換。
    const onPointerDown = (event: PointerEvent) => {
      const menu = menuRef.current
      if (menu && event.target instanceof Node && menu.contains(event.target)) return
      close()
    }
    window.addEventListener("keydown", onKeyDown)
    document.addEventListener("pointerdown", onPointerDown)
    window.addEventListener("resize", close)
    window.addEventListener("blur", close)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
      document.removeEventListener("pointerdown", onPointerDown)
      window.removeEventListener("resize", close)
      window.removeEventListener("blur", close)
    }
  }, [kind, close])

  if (!kind) return null

  const entries = CONTEXT_MENU_DEFS[kind]
  const { left, top } = placeMenu(x, y, entries.length, bodyZoom(), window.innerWidth, window.innerHeight)

  return (
    <div
      ref={menuRef}
      role="menu"
      data-testid="context-menu"
      data-kind={kind}
      className="yz-pop fixed z-[80] flex w-[212px] flex-col rounded-[12px] border border-(--line-2) bg-(--frost-light) p-[5px] shadow-(--shadow-xl) [backdrop-filter:var(--blur-frost)]"
      style={{ left, top }}
      onContextMenu={(event) => {
        event.preventDefault()
        event.stopPropagation()
      }}
    >
      {entries.map((entry, index) =>
        entry === "separator" ? (
          <div
            key={`separator-${index}`}
            role="separator"
            aria-hidden="true"
            className="mx-[8px] my-[4px] h-px shrink-0 bg-(--line-1)"
          />
        ) : (
          <button
            key={entry.id}
            type="button"
            role="menuitem"
            onClick={() => runContextMenuAction(kind, entry.id, payload)}
            className={cn(
              "flex h-[31px] shrink-0 items-center rounded-[8px] px-[12px] text-left text-[12.5px] font-medium transition-colors duration-100 hover:bg-(--yz-hover)",
              entry.danger ? "text-[#c2293f]" : "text-(--ink-1)"
            )}
          >
            {entry.label}
          </button>
        )
      )}
    </div>
  )
}
