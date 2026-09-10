import { rm } from "node:fs/promises"

export async function removeRuntimeFixture(root: string): Promise<void> {
  // Bun 1.3.14 parses rm's maxRetries/retryDelay but does not apply them.
  // Keep the retry here so native Windows teardown still fails on a stuck lock.
  for (let attempt = 0; ; attempt++) {
    try {
      await rm(root, { recursive: true, force: true })
      return
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code
      if (attempt >= 10 || !["EBUSY", "ENOTEMPTY", "EPERM", "EACCES"].includes(code ?? "")) throw error
      await new Promise(resolve => setTimeout(resolve, 100 * (attempt + 1)))
    }
  }
}
