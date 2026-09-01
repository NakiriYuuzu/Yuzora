import {
  chmodSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { spawnSync } from "node:child_process"
import { describe, expect, it } from "vitest"

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..")
const adapter = join(pluginRoot, "adapters/pi/yuzora-herdr-wsl.ts")

const describePosix = process.platform === "win32" ? describe.skip : describe

describePosix("Pi adapter reporter process lifecycle", () => {
  it("reaps the reporter process group before retrying after a timeout", () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-pi-reporter-timeout-"))
    const reporter = join(dir, "reporter.sh")
    const attemptFile = join(dir, "attempted")
    const childPidFile = join(dir, "child.pid")
    const harness = join(dir, "harness.ts")

    writeFileSync(
      reporter,
      `#!/bin/sh
set -eu
if [ ! -f "$YUZORA_TEST_ATTEMPT_FILE" ]; then
  : > "$YUZORA_TEST_ATTEMPT_FILE"
  sleep 30 &
  child=$!
  printf '%s\n' "$child" > "$YUZORA_TEST_CHILD_PID_FILE"
  i=0
  while [ "$i" -lt 20000 ]; do
    printf 'blocked reporter stderr %s\n' "$i" >&2
    i=$((i + 1))
  done
  wait "$child"
fi
exit 0
`
    )
    chmodSync(reporter, 0o755)

    writeFileSync(
      harness,
      `import { readFileSync } from "node:fs"
import process from "node:process"

process.env.HERDR_ENV = "1"
process.env.HERDR_PANE_ID = "probe:p1"
process.env.YUZORA_HERDR_WSL_REPORT = ${JSON.stringify(reporter)}
process.env.YUZORA_TEST_ATTEMPT_FILE = ${JSON.stringify(attemptFile)}
process.env.YUZORA_TEST_CHILD_PID_FILE = ${JSON.stringify(childPidFile)}

const handlers = new Map<string, Array<(event?: unknown, ctx?: unknown) => void>>()
const pi = {
  events: { on() {} },
  on(name: string, handler: (event?: unknown, ctx?: unknown) => void) {
    handlers.set(name, [...(handlers.get(name) ?? []), handler])
  }
}
const extension = await import(${JSON.stringify(adapter)})
extension.default(pi)
for (const handler of handlers.get("session_start") ?? []) {
  handler({}, { mode: "tui", isIdle: () => true })
}
await Bun.sleep(1700)
const childPid = Number(readFileSync(${JSON.stringify(childPidFile)}, "utf8").trim())
let survivor = false
try {
  process.kill(childPid, 0)
  survivor = true
} catch {}
if (survivor) {
  try { process.kill(childPid, "SIGKILL") } catch {}
}
console.log(JSON.stringify({ childPid, survivor }))
process.exit(survivor ? 1 : 0)
`
    )

    try {
      const result = spawnSync("bun", [harness], {
        encoding: "utf8",
        timeout: 5000
      })
      expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(0)
      expect(JSON.parse(result.stdout.trim())).toMatchObject({ survivor: false })
      expect(readFileSync(attemptFile, "utf8")).toBe("")
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it("awaits release reporting during Pi session shutdown without orphaning descendants", () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-pi-reporter-shutdown-"))
    const reporter = join(dir, "reporter.sh")
    const releaseAttemptFile = join(dir, "release-attempted")
    const childPidFile = join(dir, "release-child.pid")
    const harness = join(dir, "harness.ts")

    writeFileSync(
      reporter,
      `#!/bin/sh
set -eu
if [ "\${1:-}" = "report-agent" ]; then
  exit 0
fi
if [ "\${1:-}" = "release-agent" ] && [ ! -f "$YUZORA_TEST_RELEASE_ATTEMPT_FILE" ]; then
  : > "$YUZORA_TEST_RELEASE_ATTEMPT_FILE"
  sleep 30 &
  child=$!
  printf '%s\n' "$child" > "$YUZORA_TEST_CHILD_PID_FILE"
  wait "$child"
fi
exit 0
`
    )
    chmodSync(reporter, 0o755)

    writeFileSync(
      harness,
      `import { existsSync, readFileSync } from "node:fs"
import process from "node:process"

process.env.HERDR_ENV = "1"
process.env.HERDR_PANE_ID = "probe:p1"
process.env.YUZORA_HERDR_WSL_REPORT = ${JSON.stringify(reporter)}
process.env.YUZORA_TEST_RELEASE_ATTEMPT_FILE = ${JSON.stringify(releaseAttemptFile)}
process.env.YUZORA_TEST_CHILD_PID_FILE = ${JSON.stringify(childPidFile)}

const originalBeforeExit = new Set(process.listeners("beforeExit"))
const originalExit = new Set(process.listeners("exit"))
type Handler = (event?: unknown, ctx?: unknown) => unknown
const handlers = new Map<string, Handler[]>()
const pi = {
  events: { on() {} },
  on(name: string, handler: Handler) {
    handlers.set(name, [...(handlers.get(name) ?? []), handler])
  }
}
const extension = await import(${JSON.stringify(adapter)})
extension.default(pi)
const addedBeforeExit = process.listeners("beforeExit").filter(
  (listener) => !originalBeforeExit.has(listener)
)
const addedExit = process.listeners("exit").filter((listener) => !originalExit.has(listener))
for (const listener of addedBeforeExit) process.removeListener("beforeExit", listener)
for (const listener of addedExit) process.removeListener("exit", listener)
for (const handler of handlers.get("session_start") ?? []) {
  await handler({}, { mode: "tui", isIdle: () => true })
}
await Bun.sleep(100)
const shutdownHandlers = handlers.get("session_shutdown") ?? []
await Promise.all(shutdownHandlers.map((handler) => handler({ reason: "quit" }, {})))
const childPid = existsSync(${JSON.stringify(childPidFile)})
  ? Number(readFileSync(${JSON.stringify(childPidFile)}, "utf8").trim())
  : null
let survivor = false
if (childPid) {
  try {
    process.kill(childPid, 0)
    survivor = true
  } catch {}
}
if (survivor && childPid) {
  try { process.kill(childPid, "SIGKILL") } catch {}
}
console.log(JSON.stringify({
  shutdownHandlers: shutdownHandlers.length,
  addedBeforeExit: addedBeforeExit.length,
  addedExit: addedExit.length,
  childPid,
  survivor
}))
process.exitCode =
  shutdownHandlers.length === 1 &&
  addedBeforeExit.length === 0 &&
  addedExit.length === 0 &&
  childPid &&
  !survivor
    ? 0
    : 1
`
    )

    try {
      const result = spawnSync("bun", [harness], {
        encoding: "utf8",
        timeout: 5000
      })
      expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(0)
      expect(JSON.parse(result.stdout.trim())).toMatchObject({
        shutdownHandlers: 1,
        addedBeforeExit: 0,
        addedExit: 0,
        survivor: false
      })
      expect(readFileSync(releaseAttemptFile, "utf8")).toBe("")
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
