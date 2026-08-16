import { Bot, Database, Files, GitBranch, Server, type LucideIcon } from "lucide-react"

/** Workbench modes. ADE (Agents) is first/default; Files is second. */
export type Mode = "ade" | "files" | "git" | "database" | "ssh"

export interface ModeDefinition {
  id: Mode
  label: string
  icon: LucideIcon
}

export const MODES: ModeDefinition[] = [
  { id: "ade", label: "ADE", icon: Bot },
  { id: "files", label: "Files", icon: Files },
  { id: "git", label: "Git", icon: GitBranch },
  { id: "database", label: "Database", icon: Database },
  { id: "ssh", label: "SSH", icon: Server },
]

export const DEFAULT_MODE: Mode = "ade"

/** Normalize legacy persisted mode ids into the current Mode union. */
export function normalizeWorkbenchMode(raw: string | null | undefined): Mode {
  if (raw === "agent" || raw === "agentzone" || raw === "AgentZone") return "ade"
  if (raw === "ade" || raw === "files" || raw === "git" || raw === "database" || raw === "ssh") {
    return raw
  }
  return DEFAULT_MODE
}
