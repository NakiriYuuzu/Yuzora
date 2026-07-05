import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { EditorArea } from "@/workbench/EditorArea"

/**
 * Files mode main region — design reference 5.3/5.4. EditorArea renders the
 * 44px design tab bar (tab strip + split action) and the CodeMirror surface
 * or empty state per group; an optional PreviewPanel docks on the right.
 * Preview/terminal toggles now live in WorkspaceRail — `previewOpen` is
 * owned by AppShell and only read here to decide whether to dock the panel.
 */
interface EditorPanelProps {
  previewOpen: boolean
}

export function EditorPanel({ previewOpen }: EditorPanelProps) {
  return (
    <div className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      <div className="flex min-h-0 flex-1">
        <EditorArea />

        {previewOpen && (
          <div className="flex w-[45%] shrink-0 flex-col border-l border-(--line-1)">
            <PreviewPanel />
          </div>
        )}
      </div>
    </div>
  )
}
