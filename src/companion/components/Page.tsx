import type { ReactNode } from "react"
import { Loader2 } from "lucide-react"

/** A full-height screen: a sticky title row, then content. */
export function Page({ title, leading, trailing, children }: { title: ReactNode; leading?: ReactNode; trailing?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col">
      <header className="sticky top-0 z-10 flex min-h-14 items-center gap-2 border-b bg-background/95 px-3 backdrop-blur">
        {leading}
        <h1 className="min-w-0 flex-1 truncate text-lg font-semibold">{title}</h1>
        {trailing}
      </header>
      <main className="flex flex-1 flex-col">{children}</main>
    </div>
  )
}

export function Message({ tone = "info", children }: { tone?: "info" | "warning" | "error"; children: ReactNode }) {
  const color = tone === "error" ? "border-destructive/40 bg-destructive/10 text-destructive" : tone === "warning" ? "border-[var(--term-amber)]/40 bg-[color-mix(in_srgb,var(--term-amber)_12%,transparent)]" : "bg-muted text-muted-foreground"
  return <p role={tone === "info" ? "status" : "alert"} className={`rounded-lg border px-3 py-2.5 text-sm ${color}`}>{children}</p>
}

export function Progress({ text }: { text: string }) {
  return (
    <p role="status" className="flex items-center justify-center gap-2 py-2 text-sm text-muted-foreground">
      <Loader2 className="size-4 animate-spin" aria-hidden />
      {text}
    </p>
  )
}
