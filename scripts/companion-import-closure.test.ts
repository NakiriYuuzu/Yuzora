import { existsSync, readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import ts from "typescript"
import { describe, expect, it } from "vitest"
import { COMPANION_ENTRY, companionAliases, companionImportClosure, outsideAllowedRoots, repoPath } from "./companion-import-closure"

const repo = resolve(import.meta.dirname, "..")

// P11 deletes desktop src/: the PWA build may only read its own sources, packages/shared and node_modules.
describe("Companion PWA import closure", () => {
  it("builds without an alias into desktop src/", async () => {
    const aliases = await companionAliases()
    expect(aliases.length).toBeGreaterThan(0)
    expect(aliases.map(({ replacement }) => repoPath(replacement)).filter(outsideAllowedRoots)).toEqual([])
  })

  it("reaches only src/companion, packages/shared and node_modules from the entry", async () => {
    const closure = companionImportClosure([COMPANION_ENTRY], await companionAliases())
    expect(closure.unresolved).toEqual([])
    expect(closure.unanalyzable).toEqual([])
    expect(closure.files.filter(outsideAllowedRoots)).toEqual([])
    expect(closure.sources.filter(outsideAllowedRoots)).toEqual([])
    // The walk really crosses TS imports, CSS @import and Tailwind @source into the package.
    expect(closure.files).toEqual(expect.arrayContaining([
      COMPANION_ENTRY,
      "src/companion/companion.css",
      "packages/shared/src/lib/utils.ts",
      "packages/shared/src/styles/tokens.css",
    ]))
    expect(closure.sources).toContain("packages/shared/src/components/ui/badge.tsx")
    expect(closure.packages).toEqual(expect.arrayContaining(["react", "tailwindcss"]))
  })
})

// Desktop keeps its import paths through these stubs; a regenerated file (e.g. `shadcn add`) would fork the source.
const STUBS = [
  ...["alert-dialog", "badge", "button", "checkbox", "collapsible", "input", "radio-group", "scroll-area", "tabs", "textarea"]
    .map((name) => `src/components/ui/${name}.tsx`),
  "src/lib/utils.ts",
  "src/terminal/xtermTheme.ts",
  "src/workbench/git/graphLayout.ts",
  "src/workbench/git/logColors.ts",
]

describe("desktop stubs of packages/shared", () => {
  it("only re-export their packages/shared counterpart", () => {
    for (const stub of STUBS) {
      const shared = stub.replace(/^src\//, "packages/shared/src/")
      expect(existsSync(resolve(repo, shared)), shared).toBe(true)
      const file = resolve(repo, stub)
      const kind = stub.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS
      const statements = ts.createSourceFile(file, readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, kind).statements
      expect(statements.length, stub).toBeGreaterThan(0)
      for (const statement of statements) {
        const target = ts.isExportDeclaration(statement) && statement.moduleSpecifier && ts.isStringLiteral(statement.moduleSpecifier)
          ? repoPath(resolve(dirname(file), statement.moduleSpecifier.text))
          : statement.getText()
        expect(target, stub).toBe(shared.replace(/\.tsx?$/, ""))
      }
    }
  })

  it("forwards tokens.css with a single @import", () => {
    const css = readFileSync(resolve(repo, "src/styles/tokens.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "").trim()
    expect(css).toBe('@import "../../packages/shared/src/styles/tokens.css";')
    expect(existsSync(resolve(repo, "packages/shared/src/styles/tokens.css"))).toBe(true)
  })
})
