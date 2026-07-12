import type { SlashCommand } from "@/agent/acpConnection"

export type ComposerSuggestionKind = "slash" | "skill" | "file"

export interface ComposerSuggestionRange {
  start: number
  end: number
}

export interface ComposerSuggestionTrigger {
  kind: ComposerSuggestionKind
  range: ComposerSuggestionRange
  query: string
}

export interface ComposerSuggestionState {
  trigger: ComposerSuggestionTrigger | null
  selectedIndex: number
  dismissed: boolean
}

export type ComposerSuggestionAction =
  | {
      type: "sync-trigger"
      trigger: ComposerSuggestionTrigger | null
      optionCount: number
    }
  | {
      type: "open-trigger"
      trigger: ComposerSuggestionTrigger
    }
  | { type: "options-changed"; optionCount: number }
  | { type: "move"; delta: -1 | 1; optionCount: number }
  | { type: "dismiss" }
  | { type: "reset" }

export interface AppliedComposerSuggestion {
  text: string
  caret: number
}

export interface InsertedComposerTrigger extends AppliedComposerSuggestion {
  trigger: ComposerSuggestionTrigger
}

export const INITIAL_COMPOSER_SUGGESTION_STATE: ComposerSuggestionState = {
  trigger: null,
  selectedIndex: 0,
  dismissed: false,
}

export function composerSuggestionOptionId(listboxId: string, key: string): string {
  return `${listboxId}-option-${encodeURIComponent(key)}`
}

function isSameTrigger(
  left: ComposerSuggestionTrigger | null,
  right: ComposerSuggestionTrigger | null
): boolean {
  return left === right || Boolean(
    left
    && right
    && left.kind === right.kind
    && left.query === right.query
    && left.range.start === right.range.start
    && left.range.end === right.range.end
  )
}

export function parseSlashTrigger(
  text: string,
  caret: number
): ComposerSuggestionTrigger | null {
  const trigger = parseComposerSuggestionTrigger(text, caret)
  return trigger?.kind === "slash" ? trigger : null
}

export function parseComposerSuggestionTrigger(
  text: string,
  caret: number
): ComposerSuggestionTrigger | null {
  if (!Number.isInteger(caret) || caret < 0 || caret > text.length) return null

  let tokenStart = caret
  while (tokenStart > 0 && !/\s/.test(text[tokenStart - 1] ?? "")) {
    tokenStart -= 1
  }

  const token = text[tokenStart]
  const kind: ComposerSuggestionKind | null = token === "/"
    ? "slash"
    : token === "$"
      ? "skill"
      : token === "@"
        ? "file"
        : null
  if (!kind) return null
  if (tokenStart > 0 && !/\s/.test(text[tokenStart - 1] ?? "")) return null

  const query = text.slice(tokenStart + 1, caret)
  if (/\s/.test(query)) return null
  if (
    kind === "slash"
    && (query.includes("/") || query.includes("$") || query.includes("@"))
  ) return null
  if (kind === "skill" && /[$@]/.test(query)) return null
  if (kind === "file" && query.includes("@")) return null

  return {
    kind,
    range: { start: tokenStart, end: caret },
    query,
  }
}

export function parseSkillTrigger(text: string, caret: number): ComposerSuggestionTrigger | null {
  const trigger = parseComposerSuggestionTrigger(text, caret)
  return trigger?.kind === "skill" ? trigger : null
}

export function parseFileTrigger(text: string, caret: number): ComposerSuggestionTrigger | null {
  const trigger = parseComposerSuggestionTrigger(text, caret)
  return trigger?.kind === "file" ? trigger : null
}

export function insertSlashTrigger(text: string, caret: number): InsertedComposerTrigger {
  const insertionCaret = Math.max(0, Math.min(text.length, Math.trunc(caret)))
  const needsBoundary = insertionCaret > 0 && !/\s/.test(text[insertionCaret - 1] ?? "")
  const insertion = needsBoundary ? " /" : "/"
  const triggerStart = insertionCaret + insertion.length - 1
  const nextCaret = insertionCaret + insertion.length
  return {
    text: `${text.slice(0, insertionCaret)}${insertion}${text.slice(insertionCaret)}`,
    caret: nextCaret,
    trigger: {
      kind: "slash",
      range: { start: triggerStart, end: nextCaret },
      query: "",
    },
  }
}

export function applyComposerSuggestion(
  text: string,
  trigger: ComposerSuggestionTrigger,
  replacement: string
): AppliedComposerSuggestion {
  const { start, end } = trigger.range
  if (start < 0 || end < start || end > text.length) {
    throw new RangeError("Composer suggestion trigger range is outside the current text")
  }

  return {
    text: `${text.slice(0, start)}${replacement}${text.slice(end)}`,
    caret: start + replacement.length,
  }
}

export function clampSuggestionIndex(index: number, optionCount: number): number {
  if (optionCount <= 0) return 0
  return Math.max(0, Math.min(Math.trunc(index), optionCount - 1))
}

export function moveSuggestionIndex(
  index: number,
  delta: -1 | 1,
  optionCount: number
): number {
  return clampSuggestionIndex(index + delta, optionCount)
}

export function composerSuggestionReducer(
  state: ComposerSuggestionState,
  action: ComposerSuggestionAction
): ComposerSuggestionState {
  switch (action.type) {
    case "open-trigger":
      return { trigger: action.trigger, selectedIndex: 0, dismissed: false }
    case "sync-trigger": {
      if (!action.trigger) return INITIAL_COMPOSER_SUGGESTION_STATE
      if (!isSameTrigger(state.trigger, action.trigger)) {
        return { trigger: action.trigger, selectedIndex: 0, dismissed: false }
      }
      return {
        ...state,
        trigger: action.trigger,
        selectedIndex: clampSuggestionIndex(state.selectedIndex, action.optionCount),
      }
    }
    case "options-changed":
      return {
        ...state,
        selectedIndex: clampSuggestionIndex(state.selectedIndex, action.optionCount),
      }
    case "move":
      return {
        ...state,
        selectedIndex: moveSuggestionIndex(state.selectedIndex, action.delta, action.optionCount),
      }
    case "dismiss":
      return { ...state, selectedIndex: 0, dismissed: true }
    case "reset":
      return INITIAL_COMPOSER_SUGGESTION_STATE
  }
}

export function filterComposerSuggestions<T>(
  options: readonly T[],
  query: string,
  getSearchText: (option: T) => string
): T[] {
  const normalizedQuery = query.toLowerCase()
  if (!normalizedQuery) return [...options]
  return options.filter((option) => getSearchText(option).toLowerCase().startsWith(normalizedQuery))
}

export interface AgentSkillCommand {
  rawName: string
  displayName: string
  description: string
  command: SlashCommand
}

export interface PartitionedAgentCommands {
  slashCommands: SlashCommand[]
  skills: AgentSkillCommand[]
  skillsSupported: boolean
}

export function partitionAgentCommands(
  agentId: string | undefined,
  commands: readonly SlashCommand[]
): PartitionedAgentCommands {
  const skillsSupported = agentId === "codex" || agentId === "pi"
  const slashCommands: SlashCommand[] = []
  const skills: AgentSkillCommand[] = []

  for (const command of commands) {
    let displayName: string | null = null
    if (agentId === "codex" && /^\$[^\s]+$/.test(command.name)) {
      displayName = command.name.slice(1)
    } else if (agentId === "pi" && /^skill:[^\s]+$/.test(command.name)) {
      displayName = command.name.slice("skill:".length)
    }

    if (displayName) {
      skills.push({
        rawName: command.name,
        displayName,
        description: command.description,
        command,
      })
    } else {
      slashCommands.push(command)
    }
  }

  return { slashCommands, skills, skillsSupported }
}

export function filterAgentSkills(
  skills: readonly AgentSkillCommand[],
  query: string
): AgentSkillCommand[] {
  const normalizedQuery = query.toLowerCase()
  if (!normalizedQuery) return [...skills]
  return skills.filter((skill) => skill.displayName.toLowerCase().startsWith(normalizedQuery)
    || skill.rawName.toLowerCase().startsWith(normalizedQuery))
}

export function buildSkillPromptText(
  rawName: string,
  composerText: string,
  slashCommands: readonly SlashCommand[]
): string {
  const text = stripOrdinarySlashCommandPrefix(composerText, slashCommands)
  return `/${rawName}${text ? ` ${text}` : ""}`
}

export function stripOrdinarySlashCommandPrefix(
  composerText: string,
  slashCommands: readonly SlashCommand[]
): string {
  return stripOrdinarySlashCommandPrefixWithCaret(
    composerText,
    composerText.length,
    slashCommands
  ).text
}

export function stripOrdinarySlashCommandPrefixWithCaret(
  composerText: string,
  caret: number,
  slashCommands: readonly SlashCommand[]
): AppliedComposerSuggestion {
  const boundedCaret = Math.max(0, Math.min(composerText.length, Math.trunc(caret)))
  const leadingWhitespace = composerText.length - composerText.trimStart().length
  let removedFromStart = leadingWhitespace
  let text = composerText.trim()
  if (text.startsWith("/")) {
    const tokenEnd = text.search(/\s/)
    const token = text.slice(1, tokenEnd === -1 ? text.length : tokenEnd)
    if (slashCommands.some((command) => command.name === token)) {
      if (tokenEnd === -1) {
        removedFromStart += text.length
        text = ""
      } else {
        const remainder = text.slice(tokenEnd)
        const commandSeparator = remainder.length - remainder.trimStart().length
        removedFromStart += tokenEnd + commandSeparator
        text = remainder.trim()
      }
    }
  }
  return {
    text,
    caret: Math.max(0, Math.min(text.length, boundedCaret - removedFromStart)),
  }
}
