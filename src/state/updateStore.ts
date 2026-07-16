import { check, type Update } from "@tauri-apps/plugin-updater"
import { relaunch } from "@tauri-apps/plugin-process"
import { create } from "zustand"

export type UpdateStatus =
  | "idle"
  | "checking"
  | "up-to-date"
  | "available"
  | "error"
  | "downloading"
  | "downloaded"
  | "download-error"
  | "installing"
  | "install-error"

interface UpdateState {
  status: UpdateStatus
  update: Update | null
  backgroundCheckStarted: boolean
  downloadedBytes: number
  contentLength: number | null
  checkForUpdates: (source?: "manual" | "background") => Promise<void>
  checkInBackgroundOnce: () => Promise<void>
  downloadUpdate: () => Promise<void>
  installAndRelaunch: () => Promise<void>
  reset: () => void
}

export const updateInitialState = {
  status: "idle" as UpdateStatus,
  update: null as Update | null,
  backgroundCheckStarted: false,
  downloadedBytes: 0,
  contentLength: null as number | null,
}

let activeCheck: Promise<void> | null = null
let activeDownload: Promise<void> | null = null
let activeInstall: Promise<void> | null = null
let generation = 0

export const useUpdateStore = create<UpdateState>()((set, get) => ({
  ...updateInitialState,

  checkForUpdates: (source = "manual") => {
    if (activeCheck) return activeCheck

    const requestGeneration = generation
    set({ status: "checking", update: null, downloadedBytes: 0, contentLength: null })

    const request = check()
      .then((update) => {
        if (requestGeneration !== generation) return
        set({
          status: update ? "available" : "up-to-date",
          update,
        })
      })
      .catch(() => {
        if (requestGeneration !== generation) return
        if (source === "background") {
          console.warn("Update check failed", { event: "update_check_failed" })
        }
        set({ status: "error", update: null })
      })
      .finally(() => {
        if (activeCheck === request) activeCheck = null
      })

    activeCheck = request
    return request
  },

  checkInBackgroundOnce: async () => {
    if (get().backgroundCheckStarted) return
    set({ backgroundCheckStarted: true })
    await get().checkForUpdates("background")
  },

  downloadUpdate: () => {
    if (activeDownload) return activeDownload
    const update = get().update
    if (!update) return Promise.resolve()

    const requestGeneration = generation
    let downloadedBytes = 0
    set({ status: "downloading", downloadedBytes: 0, contentLength: null })

    const request = update
      .download((event) => {
        if (requestGeneration !== generation) return
        if (event.event === "Started") {
          set({ contentLength: event.data.contentLength ?? null })
        } else if (event.event === "Progress") {
          downloadedBytes += event.data.chunkLength
          set({ downloadedBytes })
        }
      })
      .then(() => {
        if (requestGeneration !== generation) return
        set((state) => ({
          status: "downloaded",
          downloadedBytes: state.contentLength ?? state.downloadedBytes,
        }))
      })
      .catch(() => {
        if (requestGeneration !== generation) return
        set({ status: "download-error" })
      })
      .finally(() => {
        if (activeDownload === request) activeDownload = null
      })

    activeDownload = request
    return request
  },

  installAndRelaunch: () => {
    if (activeInstall) return activeInstall
    const update = get().update
    if (!update) return Promise.resolve()

    const requestGeneration = generation
    set({ status: "installing" })

    const request = update
      .install()
      .then(async () => {
        if (requestGeneration !== generation) return
        await relaunch()
      })
      .catch(() => {
        if (requestGeneration !== generation) return
        set({ status: "install-error" })
      })
      .finally(() => {
        if (activeInstall === request) activeInstall = null
      })

    activeInstall = request
    return request
  },

  reset: () => {
    generation += 1
    activeCheck = null
    activeDownload = null
    activeInstall = null
    set(updateInitialState)
  },
}))
