export const MACHINE_ERROR_CODES = [
  "machines-runtime-too-old", "machines-binary-unavailable", "machines-invalid-id", "machines-invalid-target",
  "machines-invalid-label", "machines-invalid-session", "machines-unknown-machine", "machines-disabled",
  "machines-unsupported-subcommand", "machines-auth-required", "machines-host-key", "machines-unreachable",
  "machines-remote-incompatible", "machines-remote-server-stopped", "machines-timeout", "machines-output-too-large",
  "machines-parse-failed", "machines-reconnect-unsupported-windows", "native-client-limit", "machines-local-only",
  "machines-busy", "herdr-operation-error"
] as const

export type MachineErrorTranslate = (key: string, options?: Record<string, unknown>) => string

export interface MachineErrorDescription {
  code: string | null
  message: string
  /** Raw diagnostic text, collapsed under the message by the UI. */
  detail: string | null
}

/** Split a Rust `"<code>"` / `"<code>: <detail>"` error and localise the code via `machines:errors.<code>`. */
export function parseMachineError(raw: unknown): { code: string | null; detail: string | null } {
  const text = (raw instanceof Error ? raw.message : String(raw ?? "")).trim()
  const match = /^([a-z][a-z0-9-]*)(?::\s*([\s\S]*))?$/.exec(text)
  if (match && (MACHINE_ERROR_CODES as readonly string[]).includes(match[1])) {
    return { code: match[1], detail: match[2]?.trim() || null }
  }
  return { code: null, detail: text || null }
}

export function describeMachineError(raw: unknown, t: MachineErrorTranslate): MachineErrorDescription {
  const { code, detail } = parseMachineError(raw)
  return {
    code,
    message: code ? t(`errors.${code}`) : t("errors.unknown"),
    detail
  }
}
