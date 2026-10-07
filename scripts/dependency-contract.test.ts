import { execFileSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

type Manifest = {
  packageManager?: string
  dependencies: Record<string, string>
  devDependencies: Record<string, string>
  overrides: Record<string, string>
  patchedDependencies?: Record<string, string>
}
type Step = { uses?: string; with?: Record<string, unknown> }
type Workflow = { jobs: Record<string, { steps?: Step[] }> }
const manifest = (directory: string): Manifest => JSON.parse(readFileSync(`${directory}/package.json`, "utf8"))
const root = manifest(".")

describe("dependency upgrade contracts", () => {
  it("keeps compiler roles, singleton overrides, and the released xterm patch", () => {
    expect(root.devDependencies.typescript).toBe("~6.0.3")
    expect(root.devDependencies["@typescript/native"]).toBe("npm:typescript@7.0.2")
    for (const item of [root, manifest("spikes/cm6-perf")]) {
      for (const name of ["@codemirror/state", "@codemirror/view"]) {
        expect(item.dependencies[name]).toBe(`^${item.overrides[name]}`)
      }
    }
    expect(root.devDependencies.vite).toBe(`^${root.overrides.vite}`)
    expect(root.patchedDependencies?.["@xterm/xterm@6.0.0"]).toBe("patches/@xterm%2Fxterm@6.0.0.patch")
  })

  it("keeps every direct Remotion package on one exact version with the supported lint parser", () => {
    const media = manifest("site-remotion")
    const dependencies = { ...media.dependencies, ...media.devDependencies }
    const version = dependencies.remotion
    expect(version).toMatch(/^\d+\.\d+\.\d+$/)
    for (const [name, value] of Object.entries(dependencies)) {
      if (name.startsWith("@remotion/")) expect(value, name).toBe(version)
    }
    expect(dependencies.react).toBe(dependencies["react-dom"])
    expect(dependencies.typescript).toBe("6.0.3")
    expect(dependencies["@typescript/native"]).toBe(root.devDependencies["@typescript/native"])
    expect(media.overrides["typescript-eslint"]).toBe(root.devDependencies["typescript-eslint"].replace(/^\^/, ""))
  })

  it("pins Bun consistently and supplies supported Node to frontend build jobs", () => {
    const files = ["ci", "release", "recover-stable-release", "host", "herdr-compatibility", "deploy-pages"]
    const workflows = JSON.parse(execFileSync("bun", ["-e", `
      const result = {};
      for (const name of ${JSON.stringify(files)}) {
        result[name] = Bun.YAML.parse(await Bun.file(".github/workflows/" + name + ".yml").text());
      }
      console.log(JSON.stringify(result));
    `], { encoding: "utf8" })) as Record<string, Workflow>
    const bunVersion = root.packageManager?.replace(/^bun@/, "")
    const nodeVersion = readFileSync(".node-version", "utf8").trim()
    for (const workflow of Object.values(workflows)) {
      for (const job of Object.values(workflow.jobs)) {
        for (const step of job.steps ?? []) {
          if (step.uses?.startsWith("oven-sh/setup-bun@")) expect(step.with?.["bun-version"]).toBe(bunVersion)
          if (step.uses?.startsWith("actions-rust-lang/setup-rust-toolchain@")) expect(step.with?.["build-warnings"]).toBe("")
          if (step.with?.toolchain) expect(step.with.toolchain).toBe("1.98.1")
        }
      }
    }
    for (const [file, job] of [["ci", "frontend-checks"], ["ci", "frontend-tests"], ["ci", "release-candidate"], ["release", "build"], ["deploy-pages", "deploy"]]) {
      const setup = workflows[file].jobs[job].steps?.find(step => step.uses?.startsWith("actions/setup-node@"))
      expect(setup?.with?.["node-version"], `${file}/${job}`).toBe(nodeVersion)
      expect(setup?.with?.["package-manager-cache"]).toBe(false)
    }
  })
})
