import { create } from "zustand"
import { writeJsonSetting } from "@/app/workbench/settingsStorage"

export interface HerdrNotificationSettings { toast: boolean; system: boolean; sound: boolean; done: boolean; blocked: boolean }
const key = "yuzora:herdr-notifications:v1"
const defaults: HerdrNotificationSettings = { toast: true, system: false, sound: false, done: true, blocked: true }
function load(): HerdrNotificationSettings {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? "{}")
    return Object.fromEntries(Object.entries(defaults).map(([name, fallback]) => [name, typeof value?.[name] === "boolean" ? value[name] : fallback])) as unknown as HerdrNotificationSettings
  } catch { return defaults }
}
export const useHerdrNotificationStore = create<HerdrNotificationSettings & { update: (patch: Partial<HerdrNotificationSettings>) => void }>((set, get) => ({
  ...load(),
  update(patch) {
    const value = { toast: get().toast, system: get().system, sound: get().sound, done: get().done, blocked: get().blocked, ...patch }
    writeJsonSetting(key, value)
    set(value)
  },
}))
