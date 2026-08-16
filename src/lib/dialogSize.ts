/**
 * Pure dialog sizing model: stable modal IDs, normalized viewport ratios,
 * localStorage persistence, and pixel clamp math. UI/hook layers call into
 * this module; nothing here touches React or DOM listeners.
 */

export const DIALOG_SIZE_STORAGE_KEY = "yuzora.dialog-sizes.v1"
export const DEFAULT_DIALOG_SIZE_RATIO = 0.8
export const DIALOG_EDGE_MARGIN_PX = 16
export const KEYBOARD_RESIZE_STEP_PX = 8
export const KEYBOARD_RESIZE_LARGE_STEP_PX = 32

export const DEFAULT_DIALOG_MIN_SIZE = {
  width: 280,
  height: 180,
} as const

export const DIALOG_SIZE_IDS = [
  "command-palette",
  "symbol-picker",
  "settings",
  "settings-install",
  "database-recovery",
  "database-connection",
  "ssh-host",
  "ssh-password",
  "askpass",
  // Alert/AlertDialog surfaces (including AppDialogHost) are intentionally
  // excluded — they remain compact and non-resizable.
  "unsaved-confirmation",
  "external-change",
  "git-rollback",
  "git-diff",
  "herdr-agent-inspector",
] as const

export type DialogSizeId = (typeof DIALOG_SIZE_IDS)[number]

export type DialogSizePreference = {
  widthRatio: number
  heightRatio: number
}

export type DialogSizeStorageV1 = {
  version: 1
  sizes: Partial<Record<DialogSizeId, DialogSizePreference>>
}

export type DialogPixelSize = {
  width: number
  height: number
}

export type DialogMinSize = {
  width: number
  height: number
}

export type DialogSizeBounds = {
  minWidth: number
  maxWidth: number
  minHeight: number
  maxHeight: number
}

const DIALOG_SIZE_ID_SET = new Set<string>(DIALOG_SIZE_IDS)

export function dialogMinSize(width: number, height: number): DialogMinSize {
  return { width, height }
}

export function isDialogSizeId(value: unknown): value is DialogSizeId {
  return typeof value === "string" && DIALOG_SIZE_ID_SET.has(value)
}

export function isValidRatio(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 && value <= 1
}

export function defaultDialogSizePreference(): DialogSizePreference {
  return {
    widthRatio: DEFAULT_DIALOG_SIZE_RATIO,
    heightRatio: DEFAULT_DIALOG_SIZE_RATIO,
  }
}

export function emptyDialogSizeStorage(): DialogSizeStorageV1 {
  return { version: 1, sizes: {} }
}

export function getViewportSize(
  source: Pick<Window, "innerWidth" | "innerHeight"> = window,
): DialogPixelSize {
  return {
    width: Math.max(1, Math.round(source.innerWidth)),
    height: Math.max(1, Math.round(source.innerHeight)),
  }
}

export function maxDialogSize(viewport: DialogPixelSize): DialogPixelSize {
  return {
    width: Math.max(1, viewport.width - DIALOG_EDGE_MARGIN_PX * 2),
    height: Math.max(1, viewport.height - DIALOG_EDGE_MARGIN_PX * 2),
  }
}

export function resolveMinSize(minSize?: DialogMinSize | null): DialogMinSize {
  const width =
    typeof minSize?.width === "number" && Number.isFinite(minSize.width)
      ? Math.max(1, Math.round(minSize.width))
      : DEFAULT_DIALOG_MIN_SIZE.width
  const height =
    typeof minSize?.height === "number" && Number.isFinite(minSize.height)
      ? Math.max(1, Math.round(minSize.height))
      : DEFAULT_DIALOG_MIN_SIZE.height
  return { width, height }
}

export function dialogSizeBounds(
  viewport: DialogPixelSize,
  minSize?: DialogMinSize | null,
): DialogSizeBounds {
  const max = maxDialogSize(viewport)
  const min = resolveMinSize(minSize)
  return {
    minWidth: Math.min(min.width, max.width),
    maxWidth: max.width,
    minHeight: Math.min(min.height, max.height),
    maxHeight: max.height,
  }
}

export function clampPixelSize(
  size: DialogPixelSize,
  viewport: DialogPixelSize,
  minSize?: DialogMinSize | null,
): DialogPixelSize {
  const bounds = dialogSizeBounds(viewport, minSize)
  return {
    width: clamp(Math.round(size.width), bounds.minWidth, bounds.maxWidth),
    height: clamp(Math.round(size.height), bounds.minHeight, bounds.maxHeight),
  }
}

export function sizeFromPreference(
  preference: DialogSizePreference,
  viewport: DialogPixelSize,
  minSize?: DialogMinSize | null,
): DialogPixelSize {
  return clampPixelSize(
    {
      width: viewport.width * preference.widthRatio,
      height: viewport.height * preference.heightRatio,
    },
    viewport,
    minSize,
  )
}

export function preferenceFromSize(
  size: DialogPixelSize,
  viewport: DialogPixelSize,
): DialogSizePreference {
  // Persist any finite positive ratio at full floating precision. Four-decimal
  // quantization collapsed ratios below 0.00005 to zero (then 80%) and made
  // keyboard 8/32px steps non-exact at some viewport widths.
  // Pixel min/max clamp remains render-only.
  return {
    widthRatio: normalizePositiveRatio(size.width / Math.max(1, viewport.width)),
    heightRatio: normalizePositiveRatio(size.height / Math.max(1, viewport.height)),
  }
}

export function normalizePreference(
  value: unknown,
  fallback: DialogSizePreference = defaultDialogSizePreference(),
): DialogSizePreference {
  if (!value || typeof value !== "object") return fallback
  const record = value as Partial<DialogSizePreference>
  return {
    widthRatio: isValidRatio(record.widthRatio)
      ? normalizePositiveRatio(record.widthRatio)
      : fallback.widthRatio,
    heightRatio: isValidRatio(record.heightRatio)
      ? normalizePositiveRatio(record.heightRatio)
      : fallback.heightRatio,
  }
}

export function parseDialogSizeStorage(raw: string | null | undefined): DialogSizeStorageV1 {
  if (!raw) return emptyDialogSizeStorage()
  try {
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== "object") return emptyDialogSizeStorage()
    const record = parsed as { version?: unknown; sizes?: unknown }
    if (record.version !== 1 || !record.sizes || typeof record.sizes !== "object") {
      return emptyDialogSizeStorage()
    }

    const sizes: DialogSizeStorageV1["sizes"] = {}
    for (const [key, value] of Object.entries(record.sizes as Record<string, unknown>)) {
      if (!isDialogSizeId(key)) continue
      const preference = normalizePreference(value, defaultDialogSizePreference())
      // Reject fully-invalid entries that collapsed to defaults only when both
      // ratios were missing/invalid — still accept partial valid preferences.
      if (
        value &&
        typeof value === "object" &&
        (isValidRatio((value as DialogSizePreference).widthRatio) ||
          isValidRatio((value as DialogSizePreference).heightRatio))
      ) {
        sizes[key] = preference
      }
    }
    return { version: 1, sizes }
  } catch {
    return emptyDialogSizeStorage()
  }
}

export function loadDialogSizePreference(
  id: DialogSizeId,
  storage: Pick<Storage, "getItem"> | null = defaultStorage(),
): DialogSizePreference {
  if (!storage) return defaultDialogSizePreference()
  try {
    const parsed = parseDialogSizeStorage(storage.getItem(DIALOG_SIZE_STORAGE_KEY))
    return parsed.sizes[id] ?? defaultDialogSizePreference()
  } catch {
    return defaultDialogSizePreference()
  }
}

export function saveDialogSizePreference(
  id: DialogSizeId,
  preference: DialogSizePreference,
  storage: Pick<Storage, "getItem" | "setItem"> | null = defaultStorage(),
): void {
  if (!storage) return
  try {
    const current = parseDialogSizeStorage(storage.getItem(DIALOG_SIZE_STORAGE_KEY))
    const next: DialogSizeStorageV1 = {
      version: 1,
      sizes: {
        ...current.sizes,
        [id]: normalizePreference(preference),
      },
    }
    storage.setItem(DIALOG_SIZE_STORAGE_KEY, JSON.stringify(next))
  } catch {
    /* private mode / quota — keep in-memory size only */
  }
}

export function clearDialogSizePreference(
  id: DialogSizeId,
  storage: Pick<Storage, "getItem" | "setItem"> | null = defaultStorage(),
): void {
  if (!storage) return
  try {
    const current = parseDialogSizeStorage(storage.getItem(DIALOG_SIZE_STORAGE_KEY))
    if (!(id in current.sizes)) return
    const sizes = { ...current.sizes }
    delete sizes[id]
    storage.setItem(
      DIALOG_SIZE_STORAGE_KEY,
      JSON.stringify({ version: 1, sizes } satisfies DialogSizeStorageV1),
    )
  } catch {
    /* ignore */
  }
}

export function applyResizeDelta(
  current: DialogPixelSize,
  delta: Partial<DialogPixelSize>,
  viewport: DialogPixelSize,
  minSize?: DialogMinSize | null,
): DialogPixelSize {
  return clampPixelSize(
    {
      width: current.width + (delta.width ?? 0),
      height: current.height + (delta.height ?? 0),
    },
    viewport,
    minSize,
  )
}

function defaultStorage(): Pick<Storage, "getItem" | "setItem"> | null {
  try {
    if (typeof localStorage === "undefined") return null
    return localStorage
  } catch {
    return null
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function normalizePositiveRatio(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return Number.MIN_VALUE
  return Math.min(1, value)
}
