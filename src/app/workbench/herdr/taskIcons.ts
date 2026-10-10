import { Bot, GitBranch, Layers, Move, Plug, Puzzle, Send, type LucideIcon } from "lucide-react"
import type { HerdrTask } from "@/state/herdrToolsStore"

export const taskIcons: Record<HerdrTask, LucideIcon> = {
  worktree: GitBranch, startAgent: Bot, messageAgent: Send, movePane: Move, sessions: Layers, integrations: Plug, plugins: Puzzle,
}
