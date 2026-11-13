import { readFileSync } from "node:fs"

import { describe, expect, it } from "bun:test"

// A GitHub Release's text is the same version's section from both files
// (app/scripts/yuzora/release.ts), so the two have to stay in step.
const CHANGELOGS = ["CHANGELOG.md", "CHANGELOG.zh-TW.md"]
const read = (path) => readFileSync(path, "utf8")

// Each version's heading (未發布 and Unreleased are the same), with the item and nested-item counts of
// each section; the first entry counts the items before the version's first "### " heading.
function outline(changelog) {
  const versions = []
  for (const line of changelog.split(/\r?\n/)) {
    if (line.startsWith("## [")) {
      versions.push({ heading: line.replace(/ - 未發布$/, " - Unreleased"), sections: [{ items: 0, nested: 0 }] })
    } else if (versions.length > 0) {
      const sections = versions[versions.length - 1].sections
      if (line.startsWith("### ")) sections.push({ items: 0, nested: 0 })
      else if (line.startsWith("- ")) sections[sections.length - 1].items += 1
      else if (/^\s+- /.test(line)) sections[sections.length - 1].nested += 1
    }
  }
  return versions
}

describe("the English and Traditional Chinese changelogs", () => {
  it("list the same versions with the same items in each section", () => {
    const english = outline(read("CHANGELOG.md"))
    const chinese = outline(read("CHANGELOG.zh-TW.md"))
    expect(english.map((v) => v.heading)).toEqual(chinese.map((v) => v.heading))
    expect(english).toEqual(chinese)
    expect(english.length).toBeGreaterThan(0)
  })

  it("link to each other at the top", () => {
    for (const path of CHANGELOGS) {
      const text = read(path)
      const top = text.slice(0, text.indexOf("## ["))
      for (const other of CHANGELOGS.filter((p) => p !== path)) {
        expect(top).toContain(`href="${other}"`)
      }
    }
  })
})
