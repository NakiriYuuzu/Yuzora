import type { HerdrRuntimeTarget } from "./herdrTypes"

/**
 * Runtime Environment identity is deliberately independent from a named Herdr
 * session. Keep all serialized/cache keys here so callers never invent their
 * own delimiter-based namespace.
 */
export const HERDR_NATIVE_RUNTIME_TARGET: HerdrRuntimeTarget = Object.freeze({
  kind: "native"
})

/** Non-secret user preference; a missing distro remains a diagnostic, never a Native fallback. */
export const HERDR_RUNTIME_TARGET_STORAGE_KEY = "yuzora.herdr.runtime-target.v1"
/** Enabled environments are persisted separately from the active selection. */
export const HERDR_ENABLED_RUNTIME_TARGETS_STORAGE_KEY = "yuzora.herdr.enabled-runtime-targets.v1"

export interface HerdrSessionRef {
  runtimeTarget: HerdrRuntimeTarget
  sessionName: string
}

export function normalizeHerdrRuntimeTarget(
  target?: HerdrRuntimeTarget | null
): HerdrRuntimeTarget {
  if (!target || target.kind === "native") return HERDR_NATIVE_RUNTIME_TARGET
  return { kind: "wsl", distro: target.distro }
}

export function isHerdrRuntimeTarget(value: unknown): value is HerdrRuntimeTarget {
  if (!value || typeof value !== "object") return false
  const candidate = value as { kind?: unknown; distro?: unknown }
  return candidate.kind === "native" ||
    (candidate.kind === "wsl" && typeof candidate.distro === "string" && candidate.distro.trim().length > 0)
}

export function loadHerdrRuntimeTarget(): HerdrRuntimeTarget {
  try {
    const raw = globalThis.localStorage?.getItem?.(HERDR_RUNTIME_TARGET_STORAGE_KEY)
    if (!raw) return HERDR_NATIVE_RUNTIME_TARGET
    const parsed: unknown = JSON.parse(raw)
    return isHerdrRuntimeTarget(parsed)
      ? normalizeHerdrRuntimeTarget(parsed)
      : HERDR_NATIVE_RUNTIME_TARGET
  } catch {
    return HERDR_NATIVE_RUNTIME_TARGET
  }
}

export function persistHerdrRuntimeTarget(target: HerdrRuntimeTarget): void {
  try {
    globalThis.localStorage?.setItem?.(
      HERDR_RUNTIME_TARGET_STORAGE_KEY,
      JSON.stringify(normalizeHerdrRuntimeTarget(target))
    )
  } catch {
    // Storage denial only removes persistence; the selected in-memory runtime remains valid.
  }
}

/**
 * Legacy settings had no enabled list. Native is always present so a corrupt or
 * absent preference cannot create an empty runtime namespace or silently turn a
 * missing WSL page into Native.
 */
export function loadEnabledHerdrRuntimeTargets(): HerdrRuntimeTarget[] {
  try {
    const raw = globalThis.localStorage?.getItem?.(HERDR_ENABLED_RUNTIME_TARGETS_STORAGE_KEY)
    if (!raw) return [HERDR_NATIVE_RUNTIME_TARGET]
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return [HERDR_NATIVE_RUNTIME_TARGET]
    const targets = parsed
      .filter(isHerdrRuntimeTarget)
      .map(normalizeHerdrRuntimeTarget)
    return dedupeHerdrRuntimeTargets([HERDR_NATIVE_RUNTIME_TARGET, ...targets])
  } catch {
    return [HERDR_NATIVE_RUNTIME_TARGET]
  }
}

export function persistEnabledHerdrRuntimeTargets(targets: HerdrRuntimeTarget[]): void {
  try {
    globalThis.localStorage?.setItem?.(
      HERDR_ENABLED_RUNTIME_TARGETS_STORAGE_KEY,
      JSON.stringify(dedupeHerdrRuntimeTargets([HERDR_NATIVE_RUNTIME_TARGET, ...targets]))
    )
  } catch {
    // Storage denial only removes persistence; the selected in-memory runtime remains valid.
  }
}

export function dedupeHerdrRuntimeTargets(targets: HerdrRuntimeTarget[]): HerdrRuntimeTarget[] {
  const seen = new Set<string>()
  return targets.filter((target) => {
    const normalized = normalizeHerdrRuntimeTarget(target)
    const key = herdrRuntimeKey(normalized)
    if (seen.has(key)) return false
    seen.add(key)
    return true
  }).map(normalizeHerdrRuntimeTarget)
}

/** Stable, display-independent key for one Runtime Environment. */
export function herdrRuntimeKey(target?: HerdrRuntimeTarget | null): string {
  const normalized = normalizeHerdrRuntimeTarget(target)
  if (normalized.kind === "native") return "native"
  return `wsl:${encodeURIComponent(normalized.distro)}`
}

/** Stable, collision-safe key for a named session within one runtime. */
export function herdrSessionKey(
  target: HerdrRuntimeTarget | null | undefined,
  sessionName: string
): string {
  return `${herdrRuntimeKey(target)}/${encodeURIComponent(sessionName)}`
}

export function sameHerdrRuntimeTarget(
  left?: HerdrRuntimeTarget | null,
  right?: HerdrRuntimeTarget | null
): boolean {
  const a = normalizeHerdrRuntimeTarget(left)
  const b = normalizeHerdrRuntimeTarget(right)
  return a.kind === b.kind && (a.kind !== "wsl" || a.distro === (b as { distro: string }).distro)
}

export function sameHerdrSessionRef(
  left: HerdrSessionRef,
  right: HerdrSessionRef
): boolean {
  return left.sessionName === right.sessionName &&
    sameHerdrRuntimeTarget(left.runtimeTarget, right.runtimeTarget)
}
