import { chmod, mkdir, mkdtemp, readdir, readFile, rm, stat, utimes, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { afterEach, describe, expect, it } from "vitest"
import { canRunGoTarget, checkNetVersionReply, collectLicenseFiles, goTarget, parseModules, renderNotices, replace, verifyNetVersion } from "./prepare-companion"

let dir = ""
afterEach(async () => {
  if (dir) await rm(dir, { recursive: true, force: true })
  dir = ""
})

describe("goTarget", () => {
  it.each([
    ["aarch64-apple-darwin", "darwin", "arm64"],
    ["x86_64-apple-darwin", "darwin", "amd64"],
    ["x86_64-pc-windows-msvc", "windows", "amd64"],
    ["aarch64-pc-windows-msvc", "windows", "arm64"],
    ["x86_64-unknown-linux-gnu", "linux", "amd64"],
    ["aarch64-unknown-linux-gnu", "linux", "arm64"],
  ])("maps %s to %s/%s", (triple, goos, goarch) => {
    expect(goTarget(triple)).toEqual({ goos, goarch })
  })

  it.each(["riscv64gc-unknown-linux-gnu", "x86_64-unknown-linux-musl", "", "constructor"])("rejects %j with a clear error", (triple) => {
    expect(() => goTarget(triple)).toThrow(`The Companion network sidecar has no Go target for ${triple}`)
  })
})

describe("replace", () => {
  it("leaves an identical file untouched", async () => {
    dir = await mkdtemp(join(tmpdir(), "yzc-replace-"))
    const file = join(dir, "bin", "sidecar")
    expect(await replace(file, Buffer.from("same"), 0o755)).toBe(true)
    // A rewrite would bring the modification time back to now.
    await utimes(file, 1_000_000_000, 1_000_000_000)
    expect(await replace(file, Buffer.from("same"), 0o755)).toBe(false)
    expect((await stat(file)).mtimeMs).toBe(1_000_000_000_000)
    expect(await readdir(join(dir, "bin"))).toEqual(["sidecar"])
  })

  it("writes changed content and leaves no temporary file", async () => {
    dir = await mkdtemp(join(tmpdir(), "yzc-replace-"))
    const file = join(dir, "notices.txt")
    await writeFile(file, "old")
    expect(await replace(file, Buffer.from("new"))).toBe(true)
    expect(await readFile(file, "utf8")).toBe("new")
    expect(await readdir(dir)).toEqual(["notices.txt"])
  })
})

describe("parseModules", () => {
  const line = (path: string, version: string, dir: string, packageDir: string) => [path, version, dir, packageDir].join("\t")

  it("keeps only versioned modules, once each, with their linked packages, in module path order", () => {
    const output = [
      "",
      line("tailscale.com", "v1.104.1", "/go/pkg/mod/tailscale.com@v1.104.1", "/go/pkg/mod/tailscale.com@v1.104.1/tsnet"),
      line("golang.org/x/sys", "v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0/unix"),
      "\t\t\t",
      line("yuzora/companion-net", "", "/work/src-tauri/companion-net", "/work/src-tauri/companion-net"),
      line("golang.org/x/sys", "v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0/unix"),
      line("golang.org/x/sys", "v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0", "/go/pkg/mod/golang.org/x/sys@v0.48.0/cpu"),
      "",
      line("filippo.io/edwards25519", "v1.2.0", "/go/pkg/mod/filippo.io/edwards25519@v1.2.0", "/go/pkg/mod/filippo.io/edwards25519@v1.2.0"),
    ].join("\n")
    expect(parseModules(output)).toEqual([
      { path: "filippo.io/edwards25519", version: "v1.2.0", dir: "/go/pkg/mod/filippo.io/edwards25519@v1.2.0", packageDirs: ["/go/pkg/mod/filippo.io/edwards25519@v1.2.0"] },
      { path: "golang.org/x/sys", version: "v0.48.0", dir: "/go/pkg/mod/golang.org/x/sys@v0.48.0", packageDirs: ["/go/pkg/mod/golang.org/x/sys@v0.48.0/unix", "/go/pkg/mod/golang.org/x/sys@v0.48.0/cpu"] },
      { path: "tailscale.com", version: "v1.104.1", dir: "/go/pkg/mod/tailscale.com@v1.104.1", packageDirs: ["/go/pkg/mod/tailscale.com@v1.104.1/tsnet"] },
    ])
  })

  it("keeps spaces and accepts CRLF line endings", () => {
    const dir = "C:\\Users\\Yuzu Hanaoka\\go\\pkg\\mod\\tailscale.com@v1.104.1"
    expect(parseModules(`${line("tailscale.com", "v1.104.1", dir, `${dir}\\tsnet`)}\r\n`)).toEqual([
      { path: "tailscale.com", version: "v1.104.1", dir, packageDirs: [`${dir}\\tsnet`] },
    ])
  })
})

describe("collectLicenseFiles", () => {
  const write = async (path: string, text: string) => {
    await mkdir(join(path, ".."), { recursive: true })
    await writeFile(path, text)
  }

  it("finds licenses between the module root and each linked package, deduplicated, shallowest first", async () => {
    dir = await mkdtemp(join(tmpdir(), "yzc-licenses-"))
    await write(join(dir, "LICENSE"), "MIT text\n")
    await write(join(dir, "NOTICE"), "attribution\n")
    await write(join(dir, "a", "COPYING"), "BSD text\n")
    await write(join(dir, "a", "README.md"), "not a license\n")
    await write(join(dir, "a", "b", "LICENSE"), "MIT text\r\n")
    await write(join(dir, "a", "b", "c", "LICENSE.txt"), "Caleb Spare, MIT\n")
    // Not on the way to a linked package.
    await write(join(dir, "z", "LICENSE"), "unlinked\n")
    await write(join(dir, "a", "d", "LICENSE"), "unlinked sibling\n")
    // A package outside the module is ignored instead of scanned.
    const files = await collectLicenseFiles(dir, [join(dir, "a", "b", "c"), join(dir, "a", "b"), join(dir, "..")])
    expect(files.map(({ name }) => name)).toEqual(["LICENSE", "NOTICE", "a/COPYING", "a/b/c/LICENSE.txt"])
    expect(files.at(-1)?.text).toContain("Caleb Spare")
  })

  it("scans only the module root for a package at the root, and reports a module without licenses as empty", async () => {
    dir = await mkdtemp(join(tmpdir(), "yzc-licenses-"))
    await write(join(dir, "sub", "LICENSE"), "nested only\n")
    expect(await collectLicenseFiles(dir, [dir])).toEqual([])
    expect((await collectLicenseFiles(dir, [join(dir, "sub")])).map(({ name }) => name)).toEqual(["sub/LICENSE"])
  })
})

describe("renderNotices", () => {
  const modules = [
    { path: "github.com/a/b", version: "v1.0.0", files: [{ name: "LICENSE", text: "MIT text\r\n" }, { name: "NOTICE", text: "attribution\n\n" }] },
    { path: "tailscale.com", version: "v1.104.1", files: [{ name: "LICENSE", text: "BSD text" }] },
  ]

  it("puts Go's license first, then each module file in order", () => {
    const text = renderNotices("Go license\n", modules)
    const headings = text.split("\n").filter((line) => line.startsWith("=== "))
    expect(headings).toEqual([
      "=== Go standard library: LICENSE ===",
      "=== github.com/a/b v1.0.0: LICENSE ===",
      "=== github.com/a/b v1.0.0: NOTICE ===",
      "=== tailscale.com v1.104.1: LICENSE ===",
    ])
    expect(text.indexOf("Go license")).toBeLessThan(text.indexOf("MIT text"))
    expect(text.indexOf("BSD text")).toBeGreaterThan(text.indexOf("attribution"))
  })

  it("is stable across line endings and trailing whitespace", () => {
    const text = renderNotices("Go license", modules)
    expect(text).not.toContain("\r")
    expect(text).toContain("=== github.com/a/b v1.0.0: LICENSE ===\n\nMIT text\n\n=== github.com/a/b v1.0.0: NOTICE ===\n\nattribution\n")
    expect(renderNotices("Go license\r\n", modules)).toBe(text)
  })
})

describe("checkNetVersionReply", () => {
  const reply = (version: string) => `${JSON.stringify({ v: 1, type: "error", code: "net-version-mismatch", message: `desktop is 0.0.0-version-probe but yuzora-companion-net is ${version}` })}\n`

  it("accepts a sidecar that names the desktop version as its own", () => {
    expect(() => checkNetVersionReply(reply("0.0.19"), "0.0.19")).not.toThrow()
    expect(() => checkNetVersionReply(`\n${reply("0.0.19-beta.1").trimEnd()}\r\n`, "0.0.19-beta.1")).not.toThrow()
  })

  it("rejects a sidecar built without the version, such as a mistyped -X flag", () => {
    expect(() => checkNetVersionReply(reply("dev"), "0.0.19")).toThrow("reports version dev but the desktop is 0.0.19")
    expect(() => checkNetVersionReply(reply("0.0.190"), "0.0.19")).toThrow("-X main.version")
    expect(() => checkNetVersionReply(reply("0.0.1"), "0.0.19")).toThrow("-X main.version")
  })

  it.each([
    ["", "no output"],
    ["not json\n", "garbage"],
    [`${JSON.stringify({ v: 1, type: "started", version: "0.0.19" })}\n`, "a started frame"],
    [`${JSON.stringify({ v: 1, type: "error", code: "net-init-invalid", message: "yuzora-companion-net is 0.0.19" })}\n`, "another error"],
  ])("rejects %j (%s)", (stdout) => {
    expect(() => checkNetVersionReply(stdout, "0.0.19")).toThrow("did not answer the version probe")
  })
})

describe("canRunGoTarget", () => {
  it("runs only a sidecar for this machine's own OS and architecture", () => {
    expect(canRunGoTarget({ goos: "darwin", goarch: "arm64" }, "darwin", "arm64")).toBe(true)
    expect(canRunGoTarget({ goos: "windows", goarch: "amd64" }, "win32", "x64")).toBe(true)
    expect(canRunGoTarget({ goos: "linux", goarch: "arm64" }, "linux", "arm64")).toBe(true)
    expect(canRunGoTarget({ goos: "darwin", goarch: "amd64" }, "darwin", "arm64")).toBe(false)
    expect(canRunGoTarget({ goos: "linux", goarch: "amd64" }, "darwin", "x64")).toBe(false)
    expect(canRunGoTarget({ goos: "windows", goarch: "arm64" }, "freebsd", "arm64")).toBe(false)
  })
})

describe.skipIf(process.platform === "win32")("verifyNetVersion", () => {
  const fakeSidecar = async (version: string) => {
    dir = await mkdtemp(join(tmpdir(), "yzc-net-version-"))
    const binary = join(dir, "yuzora-companion-net")
    // Reads the init line like the real sidecar and answers the mismatch frame.
    await writeFile(binary, `#!/bin/sh\nread line\ncase "$line" in *'"version":"0.0.0-version-probe"'*) ;; *) exit 9;; esac\nprintf '%s\\n' '{"v":1,"type":"error","code":"net-version-mismatch","message":"desktop is 0.0.0-version-probe but yuzora-companion-net is ${version}"}'\nexit 2\n`)
    await chmod(binary, 0o755)
    return binary
  }

  it("probes the binary with an init frame for a version no release has", async () => {
    const binary = await fakeSidecar("0.0.19")
    expect(() => verifyNetVersion(binary, "0.0.19")).not.toThrow()
  })

  it("fails the build for a binary that carries another version", async () => {
    const binary = await fakeSidecar("dev")
    expect(() => verifyNetVersion(binary, "0.0.19")).toThrow("reports version dev but the desktop is 0.0.19")
  })
})
