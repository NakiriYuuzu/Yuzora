/** Release drift is a failing CI gate, never automatic compatibility approval. */
const verified = new Set(['v0.8.2', 'v0.9.0'])
const response = await fetch('https://api.github.com/repos/herdrdev/herdr/releases?per_page=100', { signal: AbortSignal.timeout(15000) })
if (!response.ok) throw new Error(`HERDR release inventory HTTP ${response.status}`)
const releases = await response.json() as Array<{ tag_name: string; draft: boolean; prerelease: boolean }>
const supportedEra = releases.filter(r => !r.draft && !r.prerelease && /^v\d+\.\d+\.\d+$/.test(r.tag_name) && (() => {
  const [major, minor, patch] = r.tag_name.slice(1).split('.').map(Number)
  return major > 0 || minor > 8 || (minor === 8 && patch >= 2)
})())
const missing = supportedEra.filter(r => !verified.has(r.tag_name)).map(r => r.tag_name)
console.log(JSON.stringify({ released: supportedEra.map(r => r.tag_name), missing }, null, 2))
if (missing.length) throw new Error(`New HERDR releases require schema fixtures, compatibility matrix and native acceptance: ${missing.join(', ')}`)
