import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { isMacPlatform } from "@/lib/platform"
import { orderAgentsByRecency, useAgentMruStore } from "@/state/agentMruStore"
import {
  dispatchAppShortcut,
  effectiveBinding,
  useKeyboardSettingsStore,
  type AppCommandId,
} from "@/state/keyboardSettingsStore"

const JUMP_COMMANDS = [1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => `agent${n}` as AppCommandId)
type Modifier = "altKey" | "ctrlKey" | "metaKey" | "shiftKey"

export interface AgentSwitcherState<T> {
  items: T[]
  index: number
}

/**
 * Modifiers the user holds while cycling. Releasing all of them commits the
 * highlighted Agent. Shift only counts when the chord has no other modifier,
 * because Shift also selects the reverse direction.
 */
export function cycleHoldModifiers(binding: string, mac = isMacPlatform()): Modifier[] {
  const parts = binding.split("+").slice(0, -1)
  const held = parts.flatMap((part): Modifier[] =>
    part === "Mod" ? [mac ? "metaKey" : "ctrlKey"] : part === "Ctrl" ? ["ctrlKey"] : part === "Alt" ? ["altKey"] : [])
  return held.length ? held : parts.includes("Shift") ? ["shiftKey"] : []
}

/**
 * Agents-list hotkeys: `agent1..9` jump directly; `agentCycleNext/Previous`
 * open an Alt+Tab-style switcher ordered by recent use. Listeners run in the
 * capture phase so they also work while a terminal has focus.
 */
export function useAgentHotkeys<T>({ agents, keyOf, activate }: {
  agents: readonly T[]
  keyOf: (agent: T) => string
  activate: (agent: T) => void
}) {
  const [altHeld, setAltHeld] = useState(false)
  const [switcher, setSwitcherState] = useState<AgentSwitcherState<T> | null>(null)
  const latest = useRef({ agents, keyOf, activate, switcher })
  useLayoutEffect(() => {
    latest.current.agents = agents
    latest.current.keyOf = keyOf
    latest.current.activate = activate
  })
  const setSwitcher = useCallback((next: AgentSwitcherState<T> | null) => {
    latest.current.switcher = next
    setSwitcherState(next)
  }, [])
  const commit = useCallback((index?: number) => {
    const current = latest.current.switcher
    if (!current) return
    const item = current.items[index ?? current.index]
    setSwitcher(null)
    if (item) latest.current.activate(item)
  }, [setSwitcher])
  const highlight = useCallback((index: number) => {
    const current = latest.current.switcher
    if (current && index >= 0 && index < current.items.length) setSwitcher({ ...current, index })
  }, [setSwitcher])
  const cancel = useCallback(() => setSwitcher(null), [setSwitcher])

  useEffect(() => {
    const cycle = (direction: 1 | -1) => {
      const current = latest.current.switcher
      if (current) {
        const size = current.items.length
        setSwitcher({ ...current, index: (current.index + direction + size) % size })
        return
      }
      const { agents: list, keyOf: key } = latest.current
      const items = orderAgentsByRecency(list, key, useAgentMruStore.getState().keys)
      if (items.length < 2) return
      setSwitcher({ items, index: direction > 0 ? 1 : items.length - 1 })
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Alt") setAltHeld(true)
      else if (!event.altKey) setAltHeld(false)
      if (latest.current.switcher && event.key === "Escape") {
        event.preventDefault()
        event.stopPropagation()
        setSwitcher(null)
        return
      }
      JUMP_COMMANDS.forEach((id, index) => dispatchAppShortcut(event, id, () => {
        const agent = latest.current.agents[index]
        if (agent) latest.current.activate(agent)
      }))
      dispatchAppShortcut(event, "agentCycleNext", () => cycle(1))
      dispatchAppShortcut(event, "agentCyclePrevious", () => cycle(-1))
    }
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === "Alt") setAltHeld(false)
      if (!latest.current.switcher) return
      const hold = cycleHoldModifiers(effectiveBinding("agentCycleNext", useKeyboardSettingsStore.getState().overrides))
      if (hold.every((modifier) => !event[modifier])) commit()
    }
    const onBlur = () => {
      setAltHeld(false)
      if (latest.current.switcher) setSwitcher(null)
    }
    window.addEventListener("keydown", onKeyDown, true)
    window.addEventListener("keyup", onKeyUp, true)
    window.addEventListener("blur", onBlur)
    return () => {
      window.removeEventListener("keydown", onKeyDown, true)
      window.removeEventListener("keyup", onKeyUp, true)
      window.removeEventListener("blur", onBlur)
    }
  }, [commit, setSwitcher])

  return { altHeld, switcher, commit, highlight, cancel }
}
