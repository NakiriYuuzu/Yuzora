let nativePreviewQueue: Promise<void> = Promise.resolve()

export function enqueueNativePreviewOperation<T>(operation: () => Promise<T>): Promise<T> {
  const queued = nativePreviewQueue.then(operation, operation)
  nativePreviewQueue = queued.then(() => undefined, () => undefined)
  return queued
}
