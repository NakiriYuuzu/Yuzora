import { invokeHerdr } from "./herdrProvider"

/** Closed request union mirrors the shared Rust facade and HERDR's public schema. */
export type HerdrFeatureRequest =
  | { method: "worktree.create"; params: { workspace_id: string; branch?: string; base?: string; path?: string; label?: string; focus: boolean } }
  | { method: "worktree.open"; params: { workspace_id: string; path: string; label?: string; focus: boolean } }
  | { method: "worktree.remove"; params: { workspace_id: string; force: boolean } }
  | { method: "pane.move"; params: { pane_id: string; destination: PaneMoveDestination; focus: boolean } }
  | { method: "agent.start"; params: { pane_id: string; name: string; kind: string; args: string[]; timeout_ms: number } }
  | { method: "agent.prompt"; params: { target: string; text: string; wait?: { until: string[]; timeout_ms: number } } }
  | { method: "agent.wait"; params: { target: string; until: string[]; timeout_ms: number } }
  | { method: "agent.rename"; params: { target: string; name?: string } }
  | { method: "agent.send_keys"; params: { target: string; keys: string[] } }
  | { method: "agent.explain"; params: { target: string } }
  | { method: "integration.list" | "plugin.list" | "session.start" | "session.stop" | "session.delete"; params: Record<string, never> }
  | { method: "integration.install" | "integration.uninstall"; params: { target: string } }
  | { method: "plugin.enable" | "plugin.disable" | "plugin.log.list" | "plugin.uninstall"; params: { plugin_id: string } }
  | { method: "plugin.action.invoke"; params: { plugin_id: string; action_id: string; context?: { workspace_id: string; tab_id?: string; focused_pane_id?: string } } }
  | { method: "plugin.pane.open"; params: { plugin_id: string; entrypoint: string; placement: "overlay" | "popup" | "split" | "tab" | "zoomed"; workspace_id?: string; target_pane_id?: string; direction?: "right" | "down"; focus: boolean } }
  | { method: "plugin.install"; params: { source: string; revision?: string } }

export type PaneMoveDestination =
  | { type: "tab"; tab_id: string; target_pane_id?: string; split: "right" | "down" }
  | { type: "new_tab"; workspace_id: string; label?: string }
  | { type: "new_workspace"; label?: string; tab_label?: string }
export interface HerdrFeatureResult {
  type?: string
  messages?: string[]
  text?: string
  [key: string]: unknown
}
export interface HerdrIntegration {
  target: string; label: string; command: string; available: boolean
  state: "not_installed" | "current" | "outdated"
}
export interface HerdrPlugin {
  plugin_id: string; name: string; version: string; description?: string; enabled: boolean
  actions?: { id: string; title: string; description?: string }[]
  panes?: { id: string; title: string; placement?: "overlay" | "popup" | "split" | "tab" | "zoomed" }[]
}

export async function herdrFeature<T extends HerdrFeatureResult = HerdrFeatureResult>(
  sessionName: string,
  request: HerdrFeatureRequest,
): Promise<T> {
  if (!sessionName) throw new Error("HERDR Session is required")
  return invokeHerdr<T>("herdr_feature", { sessionName, request })
}
