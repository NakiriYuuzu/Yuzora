import { existsSync, readFileSync, readdirSync, statSync } from "node:fs"
import { dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"
import ts from "typescript"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")
export const COMPANION_ENTRY = "src/companion/main.tsx"
/** The only repo roots the PWA build may read once desktop `src/` is deleted (P11); packages come from node_modules. */
export const ALLOWED_ROOTS = ["src/companion/", "packages/shared/"]
// Vite's default `resolve.extensions`, in its order.
const EXTENSIONS = [".mjs", ".js", ".mts", ".ts", ".jsx", ".tsx", ".json"]
const SCRIPT = /\.[cm]?[jt]sx?$/

export type Alias = { find: string; replacement: string }
export type Closure = {
  /** Every local file the build reads, repo-relative and sorted. */
  files: string[]
  /** Bare specifiers resolved to node_modules, by package name. */
  packages: string[]
  /** Tailwind `@source` targets (files or directories), repo-relative. */
  sources: string[]
  /** `importer: specifier` pairs that resolve to nothing. */
  unresolved: string[]
  /** `file:line: what` for loads no literal path names (fail closed). */
  unanalyzable: string[]
}

/** The aliases `vite.companion.config.ts` builds the PWA with. */
export async function companionAliases(): Promise<Alias[]> {
  const { default: config } = await import("../vite.companion.config")
  const alias = config.resolve?.alias ?? {}
  if (Array.isArray(alias)) return alias.map(({ find, replacement }) => ({ find: String(find), replacement }))
  return Object.entries(alias as Record<string, string>).map(([find, replacement]) => ({ find, replacement }))
}

export function repoPath(file: string): string {
  return relative(repo, file).split("\\").join("/")
}

function packageName(specifier: string): string {
  const parts = specifier.split("/")
  return specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0]
}

function asFile(candidate: string): string | undefined {
  const isFile = (path: string) => existsSync(path) && statSync(path).isFile()
  if (isFile(candidate)) return candidate
  for (const extension of EXTENSIONS) if (isFile(candidate + extension)) return candidate + extension
  for (const extension of EXTENSIONS) if (isFile(join(candidate, `index${extension}`))) return join(candidate, `index${extension}`)
  return undefined
}

type Resolved = { file: string } | { package: string } | undefined

function resolveSpecifier(raw: string, importer: string, aliases: Alias[]): Resolved {
  const specifier = raw.replace(/[?#].*$/, "")
  if (specifier.startsWith(".")) {
    const file = asFile(resolve(dirname(importer), specifier))
    return file ? { file } : undefined
  }
  for (const { find, replacement } of aliases) {
    if (specifier === find || specifier.startsWith(`${find}/`)) {
      const file = asFile(replacement + specifier.slice(find.length))
      return file ? { file } : undefined
    }
  }
  const name = packageName(specifier)
  return existsSync(join(repo, "node_modules", name, "package.json")) ? { package: name } : undefined
}

type References = { specifiers: string[]; sources: string[]; unanalyzable: string[] }

function scriptKind(file: string): ts.ScriptKind {
  if (file.endsWith(".tsx")) return ts.ScriptKind.TSX
  if (file.endsWith(".jsx")) return ts.ScriptKind.JSX
  return /\.[cm]?js$/.test(file) ? ts.ScriptKind.JS : ts.ScriptKind.TS
}

/**
 * Walks the AST: static and dynamic imports, every re-export form, `require`,
 * `import("…")` types, `new URL(…, import.meta.url)`. What Vite could load but
 * no literal names (template or variable specifiers, `import.meta.glob`) fails
 * closed as unanalyzable.
 */
function scriptReferences(file: string, text: string): References {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, scriptKind(file))
  const specifiers = source.referencedFiles.map((ref) => ref.fileName)
  const unanalyzable: string[] = []
  const flag = (node: ts.Node, what: string) =>
    unanalyzable.push(`${repoPath(file)}:${source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1}: ${what}`)
  const literal = (node: ts.Node | undefined) =>
    node && (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) ? node.text : undefined
  const add = (node: ts.Node, argument: ts.Node | undefined, what: string) => {
    const specifier = literal(argument)
    if (specifier === undefined) flag(node, `${what} without a string literal`)
    else specifiers.push(specifier)
  }
  const importMeta = (node: ts.Node) =>
    ts.isMetaProperty(node) && node.keywordToken === ts.SyntaxKind.ImportKeyword && node.name.text === "meta"
  const visit = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node)) add(node, node.moduleSpecifier, "import")
    else if (ts.isExportDeclaration(node) && node.moduleSpecifier) add(node, node.moduleSpecifier, "export … from")
    else if (ts.isExternalModuleReference(node)) add(node, node.expression, "import = require()")
    else if (ts.isImportTypeNode(node)) add(node, ts.isLiteralTypeNode(node.argument) ? node.argument.literal : undefined, "import() type")
    else if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword) add(node, node.arguments[0], "import()")
    else if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "require") add(node, node.arguments[0], "require()")
    else if (ts.isPropertyAccessExpression(node) && importMeta(node.expression) && node.name.text.startsWith("glob")) flag(node, `import.meta.${node.name.text}`)
    else if (ts.isNewExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "URL") {
      const base = node.arguments?.[1]
      if (base && ts.isPropertyAccessExpression(base) && importMeta(base.expression) && base.name.text === "url") add(node, node.arguments?.[0], "new URL(…, import.meta.url)")
    }
    ts.forEachChild(node, visit)
  }
  visit(source)
  return { specifiers, sources: [], unanalyzable }
}

/**
 * CSS imports and `url()`s, plus every path Tailwind scans: `@source` and the
 * `source(…)` of an import. A Tailwind import without `source(…)` would scan
 * the Vite root automatically, so it fails closed.
 */
function cssReferences(file: string, text: string): References {
  // Blank comments out but keep their newlines, so match offsets give real line numbers.
  const css = text.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "))
  const line = (index: number) => css.slice(0, index).split("\n").length
  const unanalyzable: string[] = []
  const sources = [...css.matchAll(/@source\s+(?:not\s+)?["']([^"']+)["']/g)].map((match) => match[1])
  for (const match of css.matchAll(/@(import|tailwind)\s+([^;]*);/g)) {
    const [, rule, params] = match
    const tailwind = rule === "tailwind" ? /^utilities\b/.test(params) : /^["']tailwindcss(?:\/utilities(?:\.css)?)?["']/.test(params)
    const source = /\bsource\(\s*(?:(none)|["']([^"']+)["'])\s*\)/.exec(params)
    if (source?.[2] !== undefined) sources.push(source[2])
    else if (!source && /\bsource\(/.test(params)) unanalyzable.push(`${repoPath(file)}:${line(match.index)}: unparsed source(…)`)
    else if (!source && tailwind) unanalyzable.push(`${repoPath(file)}:${line(match.index)}: Tailwind import without source(…) scans the Vite root`)
  }
  const specifiers = [
    ...[...css.matchAll(/@(?:import|reference|plugin|config)\s+(?:url\(\s*)?["']([^"']+)["']/g)].map((match) => match[1]),
    ...[...css.matchAll(/url\(\s*["']?([^"')\s]+)["']?\s*\)/g)].map((match) => match[1])
      .filter((url) => !/^(?:data:|https?:|#|\/)/.test(url)),
  ]
  return { specifiers: [...new Set(specifiers)], sources, unanalyzable }
}

/** Module specifiers a file pulls in, the paths Tailwind scans, and what cannot be resolved statically. */
function references(file: string): References {
  const text = readFileSync(file, "utf8")
  if (SCRIPT.test(file)) return scriptReferences(file, text)
  if (file.endsWith(".css")) return cssReferences(file, text)
  return { specifiers: [], sources: [], unanalyzable: [] }
}

/** Walks every file reachable from `entries` the way the PWA build resolves them. */
export function companionImportClosure(entries: string[], aliases: Alias[]): Closure {
  const files = new Set<string>()
  const packages = new Set<string>()
  const sources = new Set<string>()
  const unresolved: string[] = []
  const unanalyzable: string[] = []
  const queue = entries.map((entry) => resolve(repo, entry))
  while (queue.length) {
    const file = queue.shift()!
    if (files.has(file)) continue
    files.add(file)
    const found = references(file)
    unanalyzable.push(...found.unanalyzable)
    for (const source of found.sources) sources.add(repoPath(resolve(dirname(file), source)))
    for (const specifier of found.specifiers) {
      const target = resolveSpecifier(specifier, file, aliases)
      if (!target) unresolved.push(`${repoPath(file)}: ${specifier}`)
      else if ("package" in target) packages.add(target.package)
      else if (!target.file.split(sep).includes("node_modules")) queue.push(target.file)
    }
  }
  return {
    files: [...files].map(repoPath).sort(),
    packages: [...packages].sort(),
    sources: [...sources].sort(),
    unresolved: unresolved.sort(),
    unanalyzable: unanalyzable.sort(),
  }
}

export function outsideAllowedRoots(path: string): boolean {
  return !ALLOWED_ROOTS.some((root) => path.startsWith(root) || `${path}/` === root)
}

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? walk(join(dir, entry.name)) : [join(dir, entry.name)])
}

if (import.meta.main) {
  const tests = process.argv.includes("--tests")
  const companion = walk(join(repo, "src/companion")).map(repoPath).sort()
  const isTest = (file: string) => /\.test\.[jt]sx?$/.test(file)
  const entries = tests ? companion.filter(isTest) : [COMPANION_ENTRY]
  const aliases = await companionAliases()
  const closure = companionImportClosure(entries, aliases)
  const group = (file: string) => file.startsWith("src/companion/") ? "src/companion" : file.startsWith("src/") ? "desktop src" : file.split("/")[0]
  const counts: Record<string, number> = {}
  for (const file of closure.files) counts[group(file)] = (counts[group(file)] ?? 0) + 1
  console.log(JSON.stringify({
    entries: tests ? `${entries.length} test files under src/companion` : entries,
    aliases: aliases.map(({ find, replacement }) => ({ find, replacement: repoPath(replacement) })),
    counts,
    companionProductionFiles: companion.filter((file) => !isTest(file)).length,
    unreachableCompanionProductionFiles: tests ? undefined : companion.filter((file) => !isTest(file) && !closure.files.includes(file)),
    outsideAllowedRoots: [...closure.files, ...closure.sources].filter(outsideAllowedRoots),
    ...closure,
  }, null, 2))
}
