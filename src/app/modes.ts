import { Bot, Database, Files, GitBranch, TerminalSquare, type LucideIcon } from "lucide-react"

export type Mode = "files" | "git" | "database" | "ssh" | "agent"

export interface ModeDefinition {
  id: Mode
  label: string
  icon: LucideIcon
}

export const MODES: ModeDefinition[] = [
  { id: "files", label: "Files", icon: Files },
  { id: "git", label: "Git", icon: GitBranch },
  { id: "database", label: "Database", icon: Database },
  { id: "ssh", label: "SSH", icon: TerminalSquare },
  { id: "agent", label: "AgentZone", icon: Bot },
]

export const DEFAULT_MODE: Mode = "files"
