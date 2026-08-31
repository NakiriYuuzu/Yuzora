import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  symlinkSync,
  writeFileSync
} from "node:fs"
import { spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..")
const installer = join(pluginRoot, "adapters/install.sh")
const reporter = join(pluginRoot, "adapters/common/herdr-wsl-report")
const adapter = join(pluginRoot, "adapters/pi/yuzora-herdr-wsl.ts")
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
    "cp",
    "chmod",
    "rm",
    "cat",
    "head",
    "grep",
    "ln",
    "touch"
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

function runInstall(
  home: string,
  action: "install" | "status" | "uninstall",
  env: NodeJS.ProcessEnv = process.env
) {
  return spawnSync(bashPath, [installer, action, "--source-root", pluginRoot, "--home", home], {
    encoding: "utf8",
    env: { ...env }
  })
}

describe("in-distro POSIX installer", () => {
  it("ships WSL-consumed adapter files with LF-only newlines", () => {
    for (const path of [installer, reporter, adapter]) {
      const bytes = readFileSync(path)
      expect(bytes.includes(13), `${path} contains a CR byte`).toBe(false)
      expect(bytes.includes(10), `${path} contains no LF newline`).toBe(true)
    }
    for (const path of [installer, reporter]) {
      expect(readFileSync(path).subarray(0, 10).toString("ascii")).toBe("#!/bin/sh\n")
    }
  })

  it("round-trips install, current status, and uninstall without touching official hooks", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-home-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    const official = join(ext, "herdr-agent-state.ts")
    writeFileSync(official, "// installed by herdr\n// HERDR_INTEGRATION_ID=pi\n")
    chmodSync(installer, 0o755)

    expect(runInstall(home, "status").stdout.trim()).toBe("absent")

    const installed = runInstall(home, "install")
    expect(installed.status, installed.stderr).toBe(0)
    expect(runInstall(home, "status").stdout.trim()).toBe("current")
    expect(readFileSync(join(ext, "yuzora-herdr-wsl.ts"), "utf8")).toContain(
      "YUZORA_WSL_ADAPTER=pi"
    )
    expect(readFileSync(official, "utf8")).toContain("HERDR_INTEGRATION_ID=pi")

    const again = runInstall(home, "install")
    expect(again.status, again.stderr).toBe(0)
    expect(runInstall(home, "status").stdout.trim()).toBe("current")

    writeFileSync(join(ext, "yuzora-herdr-wsl.ts"), "// YUZORA_WSL_ADAPTER=pi\nchanged\n")
    expect(runInstall(home, "status").stdout.trim()).toBe("drifted")

    const uninstalled = runInstall(home, "uninstall")
    expect(uninstalled.status, uninstalled.stderr).toBe(0)
    expect(runInstall(home, "status").stdout.trim()).toBe("absent")
    expect(readFileSync(official, "utf8")).toContain("HERDR_INTEGRATION_ID=pi")

    const noop = runInstall(home, "uninstall")
    expect(noop.status, noop.stderr).toBe(0)
  })

  it("refuses to overwrite a foreign reporter and leaves it in place", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-foreign-report-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    const report = join(ext, "yuzora-herdr-wsl-report")
    writeFileSync(report, "#!/bin/sh\necho yuzora:wsl:pi\n# user owned reporter\n")
    chmodSync(installer, 0o755)

    const installed = runInstall(home, "install")
    expect(installed.status).not.toBe(0)
    expect(installed.stderr).toContain("refusing to overwrite foreign")
    expect(readFileSync(report, "utf8")).toContain("user owned reporter")
  })

  it("refuses to overwrite a foreign marker and leaves it in place", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-foreign-marker-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    const marker = join(ext, "yuzora-herdr-wsl.marker")
    writeFileSync(marker, "owned-by=someone-else\n")
    chmodSync(installer, 0o755)

    const installed = runInstall(home, "install")
    expect(installed.status).not.toBe(0)
    expect(installed.stderr).toContain("refusing to overwrite foreign")
    expect(readFileSync(marker, "utf8")).toBe("owned-by=someone-else\n")
  })

  it("uninstall removes only plugin-owned reporter and marker files", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-foreign-uninstall-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    const report = join(ext, "yuzora-herdr-wsl-report")
    const marker = join(ext, "yuzora-herdr-wsl.marker")
    const ts = join(ext, "yuzora-herdr-wsl.ts")
    writeFileSync(report, "#!/bin/sh\necho yuzora:wsl:pi\n")
    writeFileSync(marker, "not-ours\n")
    writeFileSync(ts, "export default function () {}\n")
    chmodSync(installer, 0o755)

    const uninstalled = runInstall(home, "uninstall")
    expect(uninstalled.status, uninstalled.stderr).toBe(0)
    expect(readFileSync(report, "utf8")).toContain("yuzora:wsl:pi")
    expect(readFileSync(marker, "utf8")).toBe("not-ours\n")
    expect(readFileSync(ts, "utf8")).toContain("export default")
  })

  it("does not treat a marker substring as ownership on install", () => {
    chmodSync(installer, 0o755)
    const cases = [
      {
        name: "yuzora-herdr-wsl.ts",
        body: '// note: mentions YUZORA_WSL_ADAPTER=pi in docs\nexport default function () {}\n'
      },
      {
        name: "yuzora-herdr-wsl-report",
        body: "#!/bin/sh\necho 'YUZORA_WSL_ADAPTER=pi'\n# YUZORA_WSL_ADAPTER=pi extra\n"
      },
      {
        name: "yuzora-herdr-wsl.marker",
        body: "prefix YUZORA_WSL_ADAPTER=pi\nYUZORA_WSL_ADAPTER=pi-not-ours\n"
      }
    ] as const
    for (const item of cases) {
      const home = mkdtempSync(join(tmpdir(), `yuzora-wsl-false-owned-install-${item.name}-`))
      const ext = join(home, ".pi/agent/extensions")
      mkdirSync(ext, { recursive: true })
      const path = join(ext, item.name)
      writeFileSync(path, item.body)
      const installed = runInstall(home, "install")
      expect(installed.status, `${item.name}: ${installed.stderr}`).not.toBe(0)
      expect(installed.stderr).toContain("refusing to overwrite foreign")
      expect(readFileSync(path, "utf8")).toBe(item.body)
    }
  })

  it("does not delete files that only mention the marker substring on uninstall", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-false-owned-uninstall-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    const ts = join(ext, "yuzora-herdr-wsl.ts")
    const report = join(ext, "yuzora-herdr-wsl-report")
    const marker = join(ext, "yuzora-herdr-wsl.marker")
    const tsBody = 'const note = "YUZORA_WSL_ADAPTER=pi"\n'
    const reportBody = "#!/bin/sh\n# see YUZORA_WSL_ADAPTER=pi\n"
    const markerBody = "YUZORA_WSL_ADAPTER=pi \n"
    writeFileSync(ts, tsBody)
    writeFileSync(report, reportBody)
    writeFileSync(marker, markerBody)
    chmodSync(installer, 0o755)

    const uninstalled = runInstall(home, "uninstall")
    expect(uninstalled.status, uninstalled.stderr).toBe(0)
    expect(readFileSync(ts, "utf8")).toBe(tsBody)
    expect(readFileSync(report, "utf8")).toBe(reportBody)
    expect(readFileSync(marker, "utf8")).toBe(markerBody)
  })

  it("reports drifted when the marker sentinel is removed from otherwise valid files", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-status-sentinel-"))
    const ext = join(home, ".pi/agent/extensions")
    mkdirSync(ext, { recursive: true })
    chmodSync(installer, 0o755)
    expect(runInstall(home, "install").status).toBe(0)
    expect(runInstall(home, "status").stdout.trim()).toBe("current")
    const marker = join(ext, "yuzora-herdr-wsl.marker")
    const rewritten = readFileSync(marker, "utf8")
      .split("\n")
      .filter((line) => line !== "YUZORA_WSL_ADAPTER=pi")
      .join("\n")
    writeFileSync(marker, rewritten)
    expect(runInstall(home, "status").stdout.trim()).toBe("drifted")
  })

  it("reports drifted for unowned directories and dangling symlinks, not absent", () => {
    chmodSync(installer, 0o755)
    const dirHome = mkdtempSync(join(tmpdir(), "yuzora-wsl-status-dir-"))
    const dirExt = join(dirHome, ".pi/agent/extensions")
    mkdirSync(dirExt, { recursive: true })
    mkdirSync(join(dirExt, "yuzora-herdr-wsl.ts"))
    expect(runInstall(dirHome, "status").stdout.trim()).toBe("drifted")

    const linkHome = mkdtempSync(join(tmpdir(), "yuzora-wsl-status-link-"))
    const linkExt = join(linkHome, ".pi/agent/extensions")
    mkdirSync(linkExt, { recursive: true })
    symlinkSync("/nonexistent/yuzora-herdr-wsl-report", join(linkExt, "yuzora-herdr-wsl-report"))
    expect(runInstall(linkHome, "status").stdout.trim()).toBe("drifted")

    const installOverLink = runInstall(linkHome, "install")
    expect(installOverLink.status).not.toBe(0)
    expect(installOverLink.stderr).toContain("refusing to overwrite foreign")
  })

  it("reports missing-prerequisite and refuses install without python3 or flock", () => {
    const home = mkdtempSync(join(tmpdir(), "yuzora-wsl-status-prereq-"))
    mkdirSync(join(home, ".pi/agent/extensions"), { recursive: true })
    chmodSync(installer, 0o755)
    const env = {
      ...process.env,
      PATH: makeBarePath(home),
      HOME: home
    }
    const status = runInstall(home, "status", env)
    expect(status.status, status.stderr).toBe(0)
    expect(status.stdout.trim()).toBe("missing-prerequisite")
    const installed = runInstall(home, "install", env)
    expect(installed.status).not.toBe(0)
    expect(installed.stderr).toContain("missing prerequisite: python3 or flock")
  })
})
