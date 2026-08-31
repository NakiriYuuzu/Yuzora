export const PLUGIN_ID = "yuzora-wsl-agents"
export const PLUGIN_VERSION = "0.1.0"
export const MIN_HERDR_VERSION = "0.8.2"
export const SOURCE = "yuzora:wsl:pi"
export const AGENT = "pi"
export const ADAPTER_TS_NAME = "yuzora-herdr-wsl.ts"
export const ADAPTER_REPORT_NAME = "yuzora-herdr-wsl-report"
export const ADAPTER_MARKER_NAME = "yuzora-herdr-wsl.marker"
export const CONFIG_SCHEMA_VERSION = 1
export const ENABLED_AGENT = "pi"

export const WIN32_TO_WSL_WSLENV = [
  "HERDR_ENV/u",
  "HERDR_PANE_ID/u",
  "HERDR_TAB_ID/u",
  "HERDR_WORKSPACE_ID/u",
  "HERDR_BIN_PATH/up",
  "YUZORA_HERDR_SOCKET_PATH/u",
  "YUZORA_WSL_DISTRO/u"
] as const

export const REPORTER_CHILD_WSLENV = "HERDR_SOCKET_PATH/w"

export const FORBIDDEN_REPORT_TOKENS = [
  "report-agent-session",
  "report_agent_session",
  "--agent-session-id",
  "--agent-session-path",
  "agent_session_id",
  "agent_session_path"
] as const
