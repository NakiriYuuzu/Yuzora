import { useTranslation } from "react-i18next"
import { cn } from "@/lib/utils"
import type { DiffFile, DiffLine } from "../lib/diff"

const LINE_TONE: Record<"add" | "del" | "context", string> = {
  add: "bg-[color-mix(in_srgb,var(--term-ok)_14%,transparent)]",
  del: "bg-[color-mix(in_srgb,var(--destructive)_12%,transparent)]",
  context: "",
}
const MARKER = { add: "+", del: "-", context: " " }

/** Parsed diff files as monospace hunks; wide lines scroll sideways instead of wrapping. */
export function DiffText({ files }: { files: DiffFile[] }) {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col gap-4">
      {files.map((file, index) => (
        <section key={index} className="flex flex-col gap-2">
          {(file.change !== "modified" || file.mode) && (
            <p className="flex flex-wrap items-center gap-x-2 gap-y-1 px-1 text-xs text-muted-foreground">
              {file.change !== "modified" && <span className="rounded-md bg-muted px-1.5 py-0.5 font-medium">{t(`git.diff.change.${file.change}`)}</span>}
              {(file.change === "renamed" || file.change === "copied") && <span className="min-w-0 break-all font-mono">{file.oldPath} → {file.newPath}</span>}
              {file.mode && <span className="font-mono">{t("git.diff.mode", { from: file.mode[0], to: file.mode[1] })}</span>}
            </p>
          )}
          {file.hunks.length > 0 && (
            <div className="overflow-x-auto rounded-xl border bg-card py-1 font-mono text-xs leading-5">
              <div className="min-w-max">
                {file.hunks.map((hunk, hunkIndex) => (
                  <div key={hunkIndex}>
                    <div className="bg-muted px-3 py-0.5 text-muted-foreground">{hunk.header}</div>
                    {hunk.lines.map((line, lineIndex) => <Line key={lineIndex} line={line} />)}
                  </div>
                ))}
              </div>
            </div>
          )}
        </section>
      ))}
    </div>
  )
}

function Line({ line }: { line: DiffLine }) {
  if (line.kind === "noNewline") return <div className="whitespace-pre px-3 italic text-muted-foreground">{line.text}</div>
  return (
    <div className={cn("flex", LINE_TONE[line.kind])}>
      <span aria-hidden className="w-10 shrink-0 select-none pr-1 text-right text-muted-foreground">{line.oldNo}</span>
      <span aria-hidden className="w-10 shrink-0 select-none pr-1 text-right text-muted-foreground">{line.newNo}</span>
      <span aria-hidden className="w-4 shrink-0 select-none text-center text-muted-foreground">{MARKER[line.kind]}</span>
      <span className="whitespace-pre pr-3">{line.text}</span>
    </div>
  )
}
