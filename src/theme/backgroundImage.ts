/**
 * The background image lives in IndexedDB rather than localStorage: a
 * window-sized photo would blow the localStorage quota the other settings
 * share. It is downscaled once on import, so the stored blob stays small.
 */

const MAX_EDGE = 2560
const DB_NAME = "yuzora-appearance"
const STORE = "images"
const KEY = "background"

/** Fits `width`×`height` inside a `max`-pixel square, never upscaling. */
export function fitWithin(width: number, height: number, max = MAX_EDGE): { width: number; height: number } {
  const scale = Math.min(1, max / Math.max(width, height))
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) }
}

export function hasTransparency(rgba: Uint8ClampedArray): boolean {
  for (let index = 3; index < rgba.length; index += 4) if (rgba[index] < 255) return true
  return false
}

/**
 * Decodes and re-encodes an image file no larger than the window needs: JPEG,
 * or PNG when it has transparent areas (JPEG would turn them black), so the
 * theme base shows through them instead.
 */
export async function prepareBackgroundImage(file: Blob): Promise<Blob> {
  const bitmap = await createImageBitmap(file)
  try {
    const { width, height } = fitWithin(bitmap.width, bitmap.height)
    const canvas = document.createElement("canvas")
    canvas.width = width
    canvas.height = height
    const context = canvas.getContext("2d", { willReadFrequently: true })
    if (!context) throw new Error("canvas unavailable")
    context.drawImage(bitmap, 0, 0, width, height)
    const type = hasTransparency(context.getImageData(0, 0, width, height).data) ? "image/png" : "image/jpeg"
    return await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error("encode failed")), type, 0.88)
    })
  } finally {
    bitmap.close()
  }
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1)
    request.onupgradeneeded = () => request.result.createObjectStore(STORE)
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}

/** Settles on the transaction, not the request: quota errors only surface as an abort at commit. */
async function withStore<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDb()
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = db.transaction(STORE, mode)
      const request = run(transaction.objectStore(STORE))
      transaction.oncomplete = () => resolve(request.result)
      transaction.onabort = () => reject(transaction.error ?? request.error)
      transaction.onerror = () => reject(transaction.error ?? request.error)
    })
  } finally {
    db.close()
  }
}

export async function saveBackgroundImage(image: Blob): Promise<void> {
  await withStore("readwrite", store => store.put(image, KEY))
}

export async function loadBackgroundImage(): Promise<Blob | null> {
  const image = await withStore<unknown>("readonly", store => store.get(KEY))
  return image instanceof Blob ? image : null
}

export async function clearBackgroundImage(): Promise<void> {
  await withStore("readwrite", store => store.delete(KEY))
}
