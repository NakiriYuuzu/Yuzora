import { X } from "lucide-react"
import { useTranslation } from "react-i18next"

import {
  PROJECT_COLOR_OPTIONS,
  projectGlyphOptions,
  resolveProjectPresentation,
} from "@/app/workbench/projectPresentation"
import { canonicalPathKey } from "@/lib/paths"
import { cn } from "@/lib/utils"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useUiStore } from "@/state/uiStore"

export function ProjectEditorPopover() {
  const { t } = useTranslation("workbench")
  const path = useUiStore((state) => state.projectEditorPath)
  const close = useUiStore((state) => state.closeProjectEditor)
  const presentations = useRecentWorkspacesStore((state) => state.presentations)
  const updatePresentation = useRecentWorkspacesStore((state) => state.updatePresentation)

  if (!path) return null

  const saved = presentations[canonicalPathKey(path)]
  const project = resolveProjectPresentation(path, saved)
  const glyphs = projectGlyphOptions(project.name)

  return (
    <div
      role="dialog"
      aria-label={t("projectEditor.title")}
      onClick={(event) => event.stopPropagation()}
      onContextMenu={(event) => {
        event.preventDefault()
        event.stopPropagation()
      }}
      className="fixed left-[74px] top-[60px] z-[66] flex w-[250px] flex-col overflow-hidden rounded-[14px] border border-(--line-2) bg-(--frost-light) shadow-(--shadow-xl)"
      style={{
        backdropFilter: "var(--blur-frost)",
        WebkitBackdropFilter: "var(--blur-frost)",
      }}
    >
      <div className="flex items-center gap-[9px] border-b border-(--line-1) px-[12px] py-[11px]">
        <span
          aria-hidden="true"
          className="flex size-[30px] shrink-0 items-center justify-center rounded-[10px] text-[13px] font-semibold shadow-(--shadow-xs)"
          style={{ background: project.color.background, color: project.color.foreground }}
        >
          {project.glyph}
        </span>
        <span className="flex-1 font-serif text-[14px] font-semibold text-(--ink-1)">
          {t("projectEditor.title")}
        </span>
        <button
          type="button"
          aria-label={t("projectEditor.close")}
          title={t("projectEditor.close")}
          onClick={close}
          className="flex size-[24px] items-center justify-center rounded-[7px] text-(--ink-3) transition-colors duration-150 hover:bg-(--yz-hover) hover:text-(--ink-1)"
        >
          <X className="size-[14px]" aria-hidden="true" />
        </button>
      </div>

      <div className="flex flex-col gap-[14px] p-[13px]">
        <label className="flex flex-col gap-[7px]">
          <span className="text-[9.5px] font-semibold uppercase tracking-[0.08em] text-(--ink-3)">
            {t("projectEditor.name")}
          </span>
          <input
            aria-label={t("projectEditor.name")}
            value={saved?.name ?? project.name}
            maxLength={80}
            onChange={(event) => updatePresentation(path, { name: event.currentTarget.value })}
            placeholder={t("projectEditor.namePlaceholder")}
            className="h-[32px] w-full rounded-[9px] border border-(--line-2) bg-(--yz-field) px-[11px] text-[13px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
          />
        </label>

        <fieldset>
          <legend className="mb-[7px] text-[9.5px] font-semibold uppercase tracking-[0.08em] text-(--ink-3)">
            {t("projectEditor.icon")}
          </legend>
          <div className="flex flex-wrap gap-[6px]">
            {glyphs.map((glyph) => {
              const selected = glyph === project.glyph
              return (
                <button
                  key={glyph}
                  type="button"
                  aria-label={t("projectEditor.useIcon", { glyph })}
                  aria-pressed={selected}
                  onClick={() => updatePresentation(path, { glyph })}
                  className={cn(
                    "flex size-[34px] items-center justify-center rounded-[9px] border text-[17px] font-semibold transition-all duration-150",
                    selected
                      ? "border-(--yz-accent) bg-(--yz-active) text-(--ink-1) shadow-[inset_0_0_0_0.5px_var(--yz-accent)]"
                      : "border-(--line-1) bg-(--yz-field) text-(--ink-2) hover:bg-(--yz-hover)"
                  )}
                >
                  {glyph}
                </button>
              )
            })}
          </div>
        </fieldset>

        <fieldset>
          <legend className="mb-[7px] text-[9.5px] font-semibold uppercase tracking-[0.08em] text-(--ink-3)">
            {t("projectEditor.color")}
          </legend>
          <div className="flex flex-wrap gap-[9px]">
            {PROJECT_COLOR_OPTIONS.map((color) => {
              const selected = color.id === project.color.id
              return (
                <button
                  key={color.id}
                  type="button"
                  aria-label={t("projectEditor.useColor", {
                    color: t(`projectEditor.colors.${color.id}`),
                  })}
                  aria-pressed={selected}
                  onClick={() => updatePresentation(path, { color: color.id })}
                  className={cn(
                    "size-[30px] rounded-[9px] transition-transform duration-150",
                    selected
                      ? "scale-[1.08] ring-2 ring-(--yz-accent) ring-offset-2 ring-offset-(--yz-solid)"
                      : "shadow-(--shadow-xs) hover:scale-[1.05]"
                  )}
                  style={{ background: color.background }}
                />
              )
            })}
          </div>
        </fieldset>
      </div>
    </div>
  )
}
