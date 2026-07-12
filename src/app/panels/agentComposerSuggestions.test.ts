import { describe, expect, it } from "vitest"

import {
  INITIAL_COMPOSER_SUGGESTION_STATE,
  applyComposerSuggestion,
  buildSkillPromptText,
  clampSuggestionIndex,
  composerSuggestionReducer,
  filterComposerSuggestions,
  insertSlashTrigger,
  moveSuggestionIndex,
  parseComposerSuggestionTrigger,
  parseFileTrigger,
  parseSlashTrigger,
  parseSkillTrigger,
  partitionAgentCommands,
  stripOrdinarySlashCommandPrefixWithCaret,
} from "@/app/panels/agentComposerSuggestions"

describe("agentComposerSuggestions", () => {
  it("parses slash triggers only at legal token boundaries and uses the caret as the range end", () => {
    expect(parseSlashTrigger("/fix", 4)).toEqual({
      kind: "slash",
      range: { start: 0, end: 4 },
      query: "fix",
    })
    expect(parseSlashTrigger("before /fi after", 10)).toEqual({
      kind: "slash",
      range: { start: 7, end: 10 },
      query: "fi",
    })

    expect(parseSlashTrigger("before/fix", 10)).toBeNull()
    expect(parseSlashTrigger("https://example.test", 20)).toBeNull()
    expect(parseSlashTrigger("word //fix", 10)).toBeNull()
    expect(parseSlashTrigger("/fix", -1)).toBeNull()
    expect(parseSlashTrigger("/fix", 5)).toBeNull()
  })

  it("applies only the trigger range and reports the exact next caret", () => {
    const trigger = parseSlashTrigger("before /fi\nafter", 10)
    expect(trigger).not.toBeNull()

    expect(applyComposerSuggestion("before /fi\nafter", trigger!, "/fix ")).toEqual({
      text: "before /fix \nafter",
      caret: 12,
    })
  })

  it("parses legal file and skill triggers through the same range contract", () => {
    expect(parseFileTrigger("attach @src/main", 16)).toEqual({
      kind: "file",
      range: { start: 7, end: 16 },
      query: "src/main",
    })
    expect(parseSkillTrigger("$review", 7)).toEqual({
      kind: "skill",
      range: { start: 0, end: 7 },
      query: "review",
    })
    expect(parseComposerSuggestionTrigger("email@example.com", 17)).toBeNull()
    expect(parseComposerSuggestionTrigger("cash$skill", 10)).toBeNull()
    expect(parseFileTrigger("@path with-space", 16)).toBeNull()
  })

  it("inserts an explicit slash trigger at the current caret without deleting surrounding text", () => {
    expect(insertSlashTrigger("before after", 7)).toEqual({
      text: "before /after",
      caret: 8,
      trigger: {
        kind: "slash",
        range: { start: 7, end: 8 },
        query: "",
      },
    })

    const atWordEnd = insertSlashTrigger("hello", 5)
    expect(atWordEnd).toEqual({
      text: "hello /",
      caret: 7,
      trigger: {
        kind: "slash",
        range: { start: 6, end: 7 },
        query: "",
      },
    })
    expect(parseSlashTrigger(atWordEnd.text, atWordEnd.caret)).toEqual(atWordEnd.trigger)

    const midWord = insertSlashTrigger("helloWorld", 5)
    expect(midWord.text).toBe("hello /World")
    expect(parseSlashTrigger(midWord.text, midWord.caret)).toEqual(midWord.trigger)

    const atStart = insertSlashTrigger("hello", 0)
    expect(atStart.text).toBe("/hello")
    expect(atStart.caret).toBe(1)
    expect(parseSlashTrigger(atStart.text, atStart.caret)).toEqual(atStart.trigger)
  })

  it("never exposes -1 for empty options and deterministically clamps arrow movement", () => {
    expect(clampSuggestionIndex(-1, 0)).toBe(0)
    expect(clampSuggestionIndex(9, 2)).toBe(1)
    expect(moveSuggestionIndex(0, -1, 3)).toBe(0)
    expect(moveSuggestionIndex(0, 1, 3)).toBe(1)
    expect(moveSuggestionIndex(2, 1, 3)).toBe(2)
    expect(moveSuggestionIndex(2, -1, 0)).toBe(0)
  })

  it("clamps immediately when async options shrink or are replaced", () => {
    const trigger = parseSlashTrigger("/", 1)!
    let state = composerSuggestionReducer(INITIAL_COMPOSER_SUGGESTION_STATE, {
      type: "sync-trigger",
      trigger,
      optionCount: 4,
    })
    state = composerSuggestionReducer(state, { type: "move", delta: 1, optionCount: 4 })
    state = composerSuggestionReducer(state, { type: "move", delta: 1, optionCount: 4 })
    expect(state.selectedIndex).toBe(2)

    state = composerSuggestionReducer(state, { type: "options-changed", optionCount: 1 })
    expect(state.selectedIndex).toBe(0)

    state = composerSuggestionReducer(state, { type: "options-changed", optionCount: 0 })
    expect(state.selectedIndex).toBe(0)
  })

  it("keeps dismissal for the same trigger but resets selection for a changed query", () => {
    const slash = parseSlashTrigger("/", 1)!
    const slashF = parseSlashTrigger("/f", 2)!
    let state = composerSuggestionReducer(INITIAL_COMPOSER_SUGGESTION_STATE, {
      type: "sync-trigger",
      trigger: slash,
      optionCount: 2,
    })
    state = composerSuggestionReducer(state, { type: "dismiss" })
    state = composerSuggestionReducer(state, {
      type: "sync-trigger",
      trigger: slash,
      optionCount: 2,
    })
    expect(state.dismissed).toBe(true)

    state = composerSuggestionReducer(state, {
      type: "open-trigger",
      trigger: slash,
    })
    expect(state).toMatchObject({ dismissed: false, selectedIndex: 0, trigger: slash })

    state = composerSuggestionReducer(state, { type: "dismiss" })

    state = composerSuggestionReducer(state, {
      type: "sync-trigger",
      trigger: slashF,
      optionCount: 1,
    })
    expect(state).toMatchObject({ dismissed: false, selectedIndex: 0, trigger: slashF })
  })

  it("filters case-insensitively by prefix while preserving source order", () => {
    const options = [
      { name: "Format" },
      { name: "fix" },
      { name: "review" },
    ]

    expect(filterComposerSuggestions(options, "F", (option) => option.name)).toEqual([
      options[0],
      options[1],
    ])
    expect(filterComposerSuggestions(options, "", (option) => option.name)).toEqual(options)
  })

  it("partitions only pinned Codex/Pi raw command forms and fails closed otherwise", () => {
    const commands = [
      { name: "$review", description: "Review" },
      { name: "skill:deploy", description: "Deploy" },
      { name: "fix", description: "Fix" },
      { name: "$bad name", description: "Malformed" },
    ]

    expect(partitionAgentCommands("codex", commands)).toMatchObject({
      skillsSupported: true,
      skills: [{ rawName: "$review", displayName: "review" }],
      slashCommands: [commands[1], commands[2], commands[3]],
    })
    expect(partitionAgentCommands("pi", commands)).toMatchObject({
      skillsSupported: true,
      skills: [{ rawName: "skill:deploy", displayName: "deploy" }],
      slashCommands: [commands[0], commands[2], commands[3]],
    })
    for (const agentId of ["claude", "custom", undefined]) {
      expect(partitionAgentCommands(agentId, commands)).toEqual({
        skillsSupported: false,
        skills: [],
        slashCommands: commands,
      })
    }
  })

  it("builds the exact raw skill prefix, permits skill-only, and strips an ordinary slash prefix", () => {
    const slashCommands = [{ name: "fix", description: "Fix" }]
    expect(buildSkillPromptText("$review", "", slashCommands)).toBe("/$review")
    expect(buildSkillPromptText("skill:deploy", "ship it", slashCommands)).toBe(
      "/skill:deploy ship it"
    )
    expect(buildSkillPromptText("$review", "/fix explain this", slashCommands)).toBe(
      "/$review explain this"
    )
    expect(stripOrdinarySlashCommandPrefixWithCaret(
      "/fix before  after",
      12,
      slashCommands
    )).toEqual({ text: "before  after", caret: 7 })
  })
})
