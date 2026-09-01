export type WslUncTarget = {
  distro: string
  linuxCwd: string
}

export function stripVerbatimWindowsPrefix(path: string): string {
  if (!path.startsWith("\\\\?\\")) return path
  const rest = path.slice(4)
  if (rest.slice(0, 4).toUpperCase() === "UNC\\") {
    return `\\\\${rest.slice(4)}`
  }
  if (/^[A-Za-z]:[\\/]/.test(rest)) return rest
  return path
}

export function parseWslUnc(path: string): WslUncTarget | null {
  const normalized = stripVerbatimWindowsPrefix(path).replaceAll("/", "\\")
  const withoutPrefix = normalized.startsWith("\\\\") ? normalized.slice(2) : null
  if (!withoutPrefix) return null
  const segments = withoutPrefix.split("\\").filter((segment) => segment.length > 0)
  if (segments.length < 2) return null
  const server = segments[0]
  if (
    server.toLowerCase() !== "wsl.localhost" &&
    server.toLowerCase() !== "wsl$"
  ) {
    return null
  }
  const distro = segments[1].trim()
  if (!distro) return null
  const remainder = segments.slice(2)
  const linuxCwd = remainder.length === 0 ? "/" : `/${remainder.join("/")}`
  return { distro, linuxCwd }
}

export type LinuxCwdPolicy = "workspace" | "home"

export type LaunchCwdPlan =
  | {
      kind: "unc"
      distro: string
      linuxCwd: string
    }
  | {
      kind: "windows"
      windowsPath: string
    }
  | {
      kind: "home"
    }

export function planLaunchCwd(input: {
  workspacePath: string | null
  configuredDistro: string | null
  linuxCwdPolicy: LinuxCwdPolicy
}): LaunchCwdPlan {
  if (input.linuxCwdPolicy === "home") return { kind: "home" }
  const workspacePath = input.workspacePath?.trim() || null
  if (!workspacePath) return { kind: "home" }

  const unc = parseWslUnc(workspacePath)
  if (unc) {
    if (
      input.configuredDistro &&
      unc.distro.toLowerCase() !== input.configuredDistro.toLowerCase()
    ) {
      throw new Error(
        `WSL UNC distro ${JSON.stringify(unc.distro)} does not match configured distro ${JSON.stringify(input.configuredDistro)}`
      )
    }
    return { kind: "unc", distro: unc.distro, linuxCwd: unc.linuxCwd }
  }

  return {
    kind: "windows",
    windowsPath: stripVerbatimWindowsPrefix(workspacePath)
  }
}

export function wslLaunchArgs(input: {
  distro: string | null
  linuxCwd: string | null
  kind: "shell" | "pi"
}): string[] {
  const args: string[] = []
  if (input.distro) args.push("--distribution", input.distro)
  if (input.linuxCwd) args.push("--cd", input.linuxCwd)
  if (input.kind === "pi") args.push("--", "pi")
  return args
}
