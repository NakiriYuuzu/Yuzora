type Translate = (key: string, options?: Record<string, unknown>) => string

/**
 * Companion commands fail with a kebab-case code, sometimes followed by
 * `: detail` (a refusal name, a CLI message). Known codes are localized from
 * the `companion` namespace; anything else is shown as the backend sent it.
 */
export function describeCompanionError(raw: string, t: Translate): string {
  const split = raw.indexOf(": ")
  const code = split < 0 ? raw : raw.slice(0, split)
  const detail = split < 0 ? "" : raw.slice(split + 2)
  if (code === "companion-tailscale-refused") return t(`refusal.${detail}`, { defaultValue: raw })
  if (!/^[a-z][a-z0-9-]*$/.test(code)) return raw
  const message = t(`errors.${code}`, { defaultValue: "" })
  if (!message) return raw
  return detail ? `${message} (${detail})` : message
}
