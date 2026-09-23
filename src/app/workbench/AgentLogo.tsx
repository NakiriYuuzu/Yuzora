import "./space-agent-sidebar.css"
import { agentLogoMarkup } from "./agentLogos"

/** Brand mark for a HERDR agent kind; unknown kinds fall back to a monogram. */
export function AgentLogo({ kind, label, className }: { kind: string | null; label: string; className?: string }) {
  const svg = agentLogoMarkup(kind)
  return <span
    className={`agent-logo ${className ?? ""}`}
    data-agent-kind={kind ?? "unknown"}
    aria-hidden="true"
    // Static bundled SVG files only; never runtime-provided markup.
    dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
  >{svg ? undefined : (label.trim()[0] ?? "?").toUpperCase()}</span>
}
