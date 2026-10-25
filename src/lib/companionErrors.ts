type Translate = (key: string, options?: Record<string, unknown>) => string

/**
 * Companion commands fail with a kebab-case code, sometimes followed by
 * `: detail` (a spawn message). Known codes are localized from the
 * `companion` namespace; anything else is shown as the backend sent it.
 * `port` is the Companion's configured port: a taken one is named when known.
 */
export function describeCompanionError(raw: string, t: Translate, port?: number | null): string {
  const split = raw.indexOf(": ")
  const code = split < 0 ? raw : raw.slice(0, split)
  const detail = split < 0 ? "" : raw.slice(split + 2)
  if (!/^[a-z][a-z0-9-]*$/.test(code)) return raw
  const key = code === "port-in-use" && !port ? "port-in-use-generic" : code
  const message = t(`errors.${key}`, { defaultValue: "", port })
  if (!message) return raw
  return detail ? `${message} (${detail})` : message
}
