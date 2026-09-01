import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  symlinkSync,
  writeFileSync
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { spawn, spawnSync } from "node:child_process"
import { describe, expect, it } from "vitest"
import {
  assertNoSessionIdentity,
  buildReleaseAgentArgs,
  buildReportAgentArgs,
  nextSeq
} from "../lib/reporter-args"

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..")
const reporter = join(pluginRoot, "adapters/common/herdr-wsl-report")
const LOCK_BACKENDS = new Set(["python3", "python", "flock", "node", "nodejs"])

function resolveCommand(name: string): string | null {
  const result = spawnSync("/bin/bash", ["-lc", `command -v ${name}`], { encoding: "utf8" })
  const path = result.stdout.trim()
  return result.status === 0 && path.length > 0 ? path : null
}

function makeBarePath(dir: string): string {
  const bin = join(dir, "bin")
  mkdirSync(bin, { recursive: true })
  for (const name of [
    "sha256sum",
    "shasum",
    "awk",
    "paste",
    "mktemp",
    "tr",
    "cut",
    "sed",
    "date",
    "mkdir",
    "mv",
    "chmod",
    "rm",
    "cat",
    "sleep"
  ]) {
    if (LOCK_BACKENDS.has(name)) continue
    const resolved = resolveCommand(name)
    if (!resolved) continue
    const dest = join(bin, name)
    if (!existsSync(dest)) symlinkSync(resolved, dest)
  }
  return bin
}

const bashPath = resolveCommand("bash") ?? "/bin/bash"

function runReporter(env: NodeJS.ProcessEnv, args: string[]) {
  return spawnSync(bashPath, [reporter, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...env }
  })
}

describe("reporter argv contract", () => {
  it("builds report-agent and release-agent without session identity", () => {
    const report = buildReportAgentArgs({
      paneId: "w1:p1",
      state: "working",
      seq: 42,
      message: "wait"
    })
    expect(report).toEqual([
      "pane",
      "report-agent",
      "w1:p1",
      "--source",
      "yuzora:wsl:pi",
      "--agent",
      "pi",
      "--state",
      "working",
      "--seq",
      "42",
      "--message",
      "wait"
    ])
    assertNoSessionIdentity(report)
    const release = buildReleaseAgentArgs({ paneId: "w1:p1", seq: 43 })
    expect(release[1]).toBe("release-agent")
    assertNoSessionIdentity(release)
    expect(() => assertNoSessionIdentity(["pane", "report-agent-session"])).toThrow()
  })

  it("advances seq monotonically", () => {
    expect(nextSeq(100, null)).toBe(100)
    expect(nextSeq(100, 150)).toBe(151)
    expect(nextSeq(200, 150, 1)).toBe(201)
  })
})

describe("reporter process", () => {
  it("invokes herdr.exe child with named-session routing and no session flags", () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-wsl-report-"))
    const fake = join(dir, "herdr.exe")
    const log = join(dir, "argv.log")
    const envlog = join(dir, "env.log")
    writeFileSync(
      fake,
      `#!/bin/sh\nprintf '%s\\n' "$*" >>"$FAKE_HERDR_LOG"\nprintf 'HERDR_SOCKET_PATH=%s\\n' "$HERDR_SOCKET_PATH" >>"$FAKE_HERDR_ENV"\nprintf 'WSLENV=%s\\n' "$WSLENV" >>"$FAKE_HERDR_ENV"\nexit 0\n`
    )
    chmodSync(fake, 0o755)
    const result = runReporter(
      {
        HERDR_ENV: "1",
        HERDR_PANE_ID: "w1:p2",
        HERDR_BIN_PATH: fake,
        YUZORA_HERDR_SOCKET_PATH: "C:\\Users\\x\\herdr.sock",
        YUZORA_HERDR_WSL_SEQ_DIR: join(dir, "seq"),
        WSLENV: "FOO/u:HERDR_SOCKET_PATH/up",
        FAKE_HERDR_LOG: log,
        FAKE_HERDR_ENV: envlog
      },
      ["report-agent", "--state", "working"]
    )
    expect(result.status, result.stderr).toBe(0)
    const argv = readFileSync(log, "utf8")
    expect(argv).toContain("pane report-agent w1:p2 --source yuzora:wsl:pi --agent pi --state working --seq ")
    expect(argv).not.toContain("report-agent-session")
    expect(argv).not.toContain("agent-session")
    const childEnv = readFileSync(envlog, "utf8")
    expect(childEnv).toContain("HERDR_SOCKET_PATH=C:\\Users\\x\\herdr.sock")
    expect(childEnv).toMatch(/WSLENV=.*HERDR_SOCKET_PATH\/w/)
    expect(childEnv).not.toMatch(/HERDR_SOCKET_PATH\/up/)
  })

  it("retries once on failure with a higher seq, then no-ops for Pi", () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-wsl-retry-"))
    const fake = join(dir, "herdr.exe")
    const log = join(dir, "argv.log")
    const flag = join(dir, "fail-once")
    writeFileSync(
      fake,
      `#!/bin/sh\nprintf '%s\\n' "$*" >>"$FAKE_HERDR_LOG"\nif [ ! -f "$FAKE_HERDR_FAIL_FLAG" ]; then touch "$FAKE_HERDR_FAIL_FLAG"; exit 1; fi\nexit 0\n`
    )
    chmodSync(fake, 0o755)
    const result = runReporter(
      {
        HERDR_ENV: "1",
        HERDR_PANE_ID: "w1:p3",
        HERDR_BIN_PATH: fake,
        YUZORA_HERDR_SOCKET_PATH: "marker-a",
        YUZORA_HERDR_WSL_SEQ_DIR: join(dir, "seq"),
        FAKE_HERDR_LOG: log,
        FAKE_HERDR_FAIL_FLAG: flag
      },
      ["report-agent", "--state", "blocked", "--message", "ask"]
    )
    expect(result.status).toBe(0)
    const lines = readFileSync(log, "utf8").trim().split("\n")
    expect(lines).toHaveLength(2)
    const seqs = lines.map((line) => Number(line.match(/--seq (\d+)/)?.[1]))
    expect(seqs[1]).toBeGreaterThan(seqs[0]!)
    expect(lines[0]).toContain("--state blocked")
    expect(lines[0]).toContain("--message ask")
  })

  it("persists seq across processes and serializes concurrent writers", () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-wsl-seq-"))
    const fake = join(dir, "herdr.exe")
    const log = join(dir, "argv.log")
    writeFileSync(
      fake,
      `#!/bin/sh\nprintf '%s\\n' "$*" >>"$FAKE_HERDR_LOG"\nexit 0\n`
    )
    chmodSync(fake, 0o755)
    const env = {
      HERDR_ENV: "1",
      HERDR_PANE_ID: "w1:p4",
      HERDR_BIN_PATH: fake,
      YUZORA_HERDR_SOCKET_PATH: "marker-b",
      YUZORA_HERDR_WSL_SEQ_DIR: join(dir, "seq"),
      FAKE_HERDR_LOG: log
    }
    const first = runReporter(env, ["report-agent", "--state", "idle"])
    const second = runReporter(env, ["release-agent"])
    expect(first.status).toBe(0)
    expect(second.status).toBe(0)
    const a = spawnSync("bash", [reporter, "report-agent", "--state", "working"], {
      encoding: "utf8",
      env: { ...process.env, ...env }
    })
    const b = spawnSync("bash", [reporter, "report-agent", "--state", "working"], {
      encoding: "utf8",
      env: { ...process.env, ...env }
    })
    expect(a.status).toBe(0)
    expect(b.status).toBe(0)
    const seqs = readFileSync(log, "utf8")
      .trim()
      .split("\n")
      .map((line) => Number(line.match(/--seq (\d+)/)?.[1]))
    const unique = new Set(seqs)
    expect(unique.size).toBe(seqs.length)
    expect(Math.max(...seqs)).toBeGreaterThan(Math.min(...seqs))
    expect(readFileSync(log, "utf8")).toContain("release-agent")
  })

  it("releases a killed seq lock so the next report succeeds with a higher seq", async () => {
    const dir = mkdtempSync(join(tmpdir(), "yuzora-wsl-lock-kill-"))
    const fake = join(dir, "herdr.exe")
    const log = join(dir, "argv.log")
    const held = join(dir, "held")
    writeFileSync(
      fake,
      `#!/bin/sh\nprintf '%s\\n' "$*" >>"$FAKE_HERDR_LOG"\nexit 0\n`
    )
    chmodSync(fake, 0o755)
    const env = {
      ...process.env,
      HERDR_ENV: "1",
      HERDR_PANE_ID: "w1:p9",
      HERDR_BIN_PATH: fake,
      YUZORA_HERDR_SOCKET_PATH: "marker-lock",
      YUZORA_HERDR_WSL_SEQ_DIR: join(dir, "seq"),
      YUZORA_HERDR_WSL_LOCK_HOLD_MS: "8000",
      YUZORA_HERDR_WSL_LOCK_HELD_FILE: held,
      FAKE_HERDR_LOG: log
    }
    const child = spawn("bash", [reporter, "report-agent", "--state", "working"], {
      env,
      detached: true,
      stdio: "ignore"
    })
    const started = Date.now()
    while (!existsSync(held)) {
      if (Date.now() - started > 4000) {
        try {
          process.kill(-child.pid!, "SIGTERM")
        } catch {
          child.kill("SIGTERM")
        }
        throw new Error("reporter never acquired the seq lock")
      }
      await new Promise((resolve) => setTimeout(resolve, 25))
    }
    try {
      process.kill(-child.pid!, "SIGTERM")
    } catch {
      child.kill("SIGTERM")
    }
    await Promise.race([
      new Promise<void>((resolve) => child.once("exit", () => resolve())),
      new Promise<void>((resolve) => setTimeout(resolve, 1000))
    ])
    const secondStarted = Date.now()
    const second = spawnSync("bash", [reporter, "report-agent", "--state", "idle"], {
      encoding: "utf8",
      env: {
        ...env,
        YUZORA_HERDR_WSL_LOCK_HOLD_MS: "0",
        YUZORA_HERDR_WSL_LOCK_HELD_FILE: ""
      }
    })
    expect(second.status, second.stderr).toBe(0)
    expect(Date.now() - secondStarted).toBeLessThan(2000)
    const seqs = readFileSync(log, "utf8")
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => Number(line.match(/--seq (\d+)/)?.[1]))
    expect(seqs.length).toBeGreaterThanOrEqual(1)
    expect(Math.max(...seqs)).toBeGreaterThan(0)
    if (seqs.length >= 2) {
      expect(seqs[seqs.length - 1]).toBeGreaterThan(seqs[0]!)
    }
  })

  it("no-ops without HERDR_ENV=1 so Pi is not broken", () => {
    const result = runReporter({ HERDR_ENV: "0", HERDR_PANE_ID: "w1:p1" }, [
      "report-agent",
      "--state",
      "working"
    ])
    expect(result.status).toBe(0)
  })

  it("no-ops without python3 or flock and does not create a mkdir lock", () => {
    const source = readFileSync(reporter, "utf8")
    expect(source).toContain("have_seq_lock_backend")
    expect(source).toContain("missing seq lock backend")
    expect(source).not.toContain("lock.d")
    expect(source).not.toMatch(/mkdir "\$lock_dir"/)

    const dir = mkdtempSync(join(tmpdir(), "yuzora-wsl-nolock-"))
    const fake = join(dir, "herdr.exe")
    const log = join(dir, "argv.log")
    const seqDir = join(dir, "seq")
    writeFileSync(fake, `#!/bin/sh\nprintf '%s\\n' "$*" >>"$FAKE_HERDR_LOG"\nexit 0\n`)
    chmodSync(fake, 0o755)
    const result = runReporter(
      {
        PATH: makeBarePath(dir),
        HERDR_ENV: "1",
        HERDR_PANE_ID: "w1:p8",
        HERDR_BIN_PATH: fake,
        YUZORA_HERDR_SOCKET_PATH: "marker-nolock",
        YUZORA_HERDR_WSL_SEQ_DIR: seqDir,
        FAKE_HERDR_LOG: log
      },
      ["report-agent", "--state", "working"]
    )
    expect(result.status, result.stderr).toBe(0)
    expect(result.stderr).toContain("missing seq lock backend")
    expect(existsSync(log)).toBe(false)
    if (existsSync(seqDir)) {
      const leftover = readdirSync(seqDir).filter((name) => name.endsWith(".lock.d"))
      expect(leftover).toEqual([])
    }
  })
})
