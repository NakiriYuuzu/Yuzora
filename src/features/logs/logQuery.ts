import { invoke } from "@/lib/ipc"
import type { LogExportResult, LogRecord } from "@/lib/types"

export interface LogQueryFilters {
  since?: string
  until?: string
  levels?: string[]
  kinds?: string[]
  sources?: string[]
  text?: string
  limit?: number
}

export function logQuery(filters: LogQueryFilters): Promise<LogRecord[]> {
  return invoke("log_query", { filters })
}

export function logSources(): Promise<string[]> {
  return invoke("log_sources")
}

export function logExport(dest: string, sanitize: boolean): Promise<LogExportResult> {
  return invoke("log_export", { dest, sanitize })
}

// Copy rows goes through the same Rust redaction as Export bundle (issue #41).
export function logSanitizeLines(lines: string[]): Promise<string[]> {
  return invoke("log_sanitize_lines", { lines })
}

export function getLogLevel(): Promise<string> {
  return invoke("get_log_level")
}

export function setLogLevel(level: string): Promise<void> {
  return invoke("set_log_level", { level })
}
