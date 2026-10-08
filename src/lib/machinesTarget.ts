import type { SshHost } from "@/state/sshStore"

/**
 * Official `herdr machine add` target for a saved SSH host. Key files are never
 * passed (the official CLI has no such option): users rely on ssh-agent / ~/.ssh/config.
 */
export function buildMachineTarget(host: Pick<SshHost, "host" | "user" | "port">): string {
  const name = host.host.trim()
  const user = host.user.trim()
  const prefix = user ? `${user}@` : ""
  const ipv6 = name.includes(":") && !name.startsWith("[")
  const port = host.port
  const hasPort = Number.isInteger(port) && port > 0 && port !== 22
  if (!ipv6 && !hasPort) return `${prefix}${name}`
  const bracketed = ipv6 ? `[${name}]` : name
  return `ssh://${prefix}${bracketed}${hasPort ? `:${port}` : ""}`
}
