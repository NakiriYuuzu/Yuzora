import { readFileSync } from "node:fs"

import { describe, expect, it } from "bun:test"

describe("the video sources in site-remotion", () => {
  it("keep every direct Remotion package on one exact version, and react with react-dom", () => {
    const media = JSON.parse(readFileSync("site-remotion/package.json", "utf8"))
    const dependencies = { ...media.dependencies, ...media.devDependencies }
    const version = dependencies.remotion
    expect(version).toMatch(/^\d+\.\d+\.\d+$/)
    for (const [name, value] of Object.entries(dependencies)) {
      if (name.startsWith("@remotion/")) expect(value, name).toBe(version)
    }
    expect(dependencies.react).toBe(dependencies["react-dom"])
  })
})
