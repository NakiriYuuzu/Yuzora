import { mkdir, writeFile, chmod, readdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
const targets: Record<string, [string, string]> = {
  'darwin-arm64': ['herdr-macos-aarch64', 'a5d4f4d504d8b309c91f811050559300faba31258425f53c50852fc96f6ae574'],
  'linux-x64': ['herdr-linux-x86_64', '976150a14d490c94b243ea2e1a7eb2dfb67f12e36b182db90936f6728e6aecf4'],
  'win32-x64': ['herdr-windows-x86_64.zip', '0ab3d0fe1434d55757997542b978c771d642987bb15a7130f4160f0db38821d5'],
}
const target = targets[`${process.platform}-${process.arch}`]
if (!target) throw new Error('Unsupported compatibility runner')
const root = resolve('output/herdr-compat-082')
await mkdir(root, { recursive: true })
const response = await fetch(`https://github.com/herdrdev/herdr/releases/download/v0.8.2/${target[0]}`, { signal: AbortSignal.timeout(90000) })
if (!response.ok) throw new Error(`HERDR baseline download HTTP ${response.status}`)
const bytes = Buffer.from(await response.arrayBuffer())
if (createHash('sha256').update(bytes).digest('hex') !== target[1]) throw new Error('HERDR baseline checksum mismatch')
const downloaded = resolve(root, target[0])
await writeFile(downloaded, bytes)
let binary = downloaded
if (process.platform === 'win32') {
  // Fixed local paths passed through the environment, never interpolated shell code.
  const result = spawnSync('pwsh', ['-NoProfile', '-Command', 'Expand-Archive -LiteralPath $env:YUZORA_BASELINE_ZIP -DestinationPath $env:YUZORA_BASELINE_DIR -Force'], { env: { ...process.env, YUZORA_BASELINE_ZIP: downloaded, YUZORA_BASELINE_DIR: root }, stdio: 'inherit' })
  if (result.status !== 0) throw new Error('Baseline extraction failed')
  const matches = (await readdir(root, { recursive: true })).filter(path => path.endsWith('herdr.exe')).map(path => resolve(root, path))
  if (matches.length !== 1) throw new Error('Ambiguous baseline binary')
  binary = matches[0]
} else await chmod(binary, 0o755)
const result = spawnSync('bun', ['scripts/verify-herdr-runtime.ts', binary, '0.8.2'], { stdio: 'inherit' })
if (result.status !== 0) process.exit(result.status ?? 1)
