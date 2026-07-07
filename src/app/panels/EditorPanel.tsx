import { EditorArea } from "@/workbench/EditorArea"

/**
 * Files mode main region — design reference 5.3/5.4. EditorArea renders the
 * 44px design tab bar (tab strip + split action) and the CodeMirror surface
 * or empty state per group. Preview is now a singleton tab rendered inside
 * EditorArea (not a docked side panel), so this shell just hosts EditorArea.
 */
export function EditorPanel() {
  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      <div className="flex min-h-0 flex-1">
        <EditorArea />
      </div>
    </div>
  )
}
