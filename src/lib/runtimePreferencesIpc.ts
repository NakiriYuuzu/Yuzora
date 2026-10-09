import { invoke } from "./ipc"

export interface RuntimePreferences { wslEnabled: boolean }

export function runtimePreferencesGet(): Promise<RuntimePreferences> {
  return invoke("runtime_preferences_get")
}

export function runtimePreferencesSet(wslEnabled: boolean): Promise<RuntimePreferences> {
  return invoke("runtime_preferences_set", { wslEnabled })
}
