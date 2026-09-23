const logos = import.meta.glob<string>("../../assets/agent-logos/*.svg", { query: "?raw", import: "default", eager: true })

const markup = new Map(Object.entries(logos).map(([path, svg]) => [path.slice(path.lastIndexOf("/") + 1, -4), svg]))

/** Longer aliases first so "claude code" or "factory droid" win over generic words. */
const aliases: [string, string][] = [
  ["claude", "claude"], ["anthropic", "claude"], ["codex", "codex"], ["openai", "codex"],
  ["gemini", "gemini"], ["antigravity", "agy"], ["agy", "agy"], ["cursor", "cursor"],
  ["copilot", "copilot"], ["devin", "devin"], ["cline", "cline"], ["opencode", "opencode"],
  ["mastra", "mastracode"], ["kimi", "kimi"], ["kiro", "kiro"], ["droid", "droid"], ["factory", "droid"],
  ["amp", "amp"], ["grok", "grok"], ["hermes", "hermes"], ["kilo", "kilo"], ["qoder", "qodercli"],
  ["qwen", "qwen"], ["letta", "letta"], ["maki", "maki"], ["muse", "muse"], ["omp", "omp"], ["pi", "pi"],
]

/** Resolves a HERDR agent label such as "Claude Code" or "codex" to a known agent kind. */
export function resolveAgentKind(...labels: (string | null | undefined)[]): string | null {
  for (const label of labels) {
    const words = label?.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean) ?? []
    if (!words.length) continue
    const hit = aliases.find(([alias]) => words.includes(alias) || (alias.length > 3 && words.some(word => word.startsWith(alias))))
    if (hit) return hit[1]
  }
  return null
}

export function agentLogoMarkup(kind: string | null): string | null {
  return kind ? markup.get(kind) ?? null : null
}
