// fixtures/gen-git-fixtures.ts — bun run fixtures/gen-git-fixtures.ts
// 於 fixtures/out/git-demo 建立示範 repo，供 M2 GUI 手動驗收使用。
// 產出狀態：staged 修改（src/app.ts）、unstaged 修改（README.md）、untracked（scratch.txt），
// 以及 conflict-ready 分支 feature/conflict（checklist 操作 git merge 即得真衝突）。
// 界線：所有 git 操作皆以 -C dir 指向 fixtures/out/git-demo，不碰全域 config、不碰本專案 repo。

import { rmSync, mkdirSync, writeFileSync } from "node:fs"
import { join, dirname } from "node:path"

const dir = join("fixtures", "out", "git-demo")

function git(...args: string[]) {
    const res = Bun.spawnSync(["git", "-C", dir, ...args], {
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0", LC_ALL: "C" }
    })
    if (res.exitCode !== 0) {
        const stderr = new TextDecoder().decode(res.stderr)
        throw new Error(`git ${args.join(" ")} failed (exit ${res.exitCode}): ${stderr}`)
    }
    return new TextDecoder().decode(res.stdout)
}

function write(rel: string, content: string) {
    const path = join(dir, rel)
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, content)
}

// 1) 乾淨重建目錄，git init -b main，設定 local user（不碰全域）
rmSync(dir, { recursive: true, force: true })
mkdirSync(dir, { recursive: true })
git("init", "-b", "main")
git("config", "user.name", "Yuzora Fixture")
git("config", "user.email", "fixture@yuzora.local")

// 2) 三個 commit 的基礎歷史
write("src/app.ts", "export const greet = (name: string) => `hello ${name}`\n")
git("add", ".")
git("commit", "-m", "feat: initial app entry")

write("README.md", "# git-demo\n\nYuzora M2 GUI acceptance fixture.\n")
git("add", ".")
git("commit", "-m", "docs: add readme")

write("note.txt", "line 1\nline 2\nline 3\n")
git("add", ".")
git("commit", "-m", "chore: add note")

// 3) conflict-ready 分支：feature/conflict 改 note.txt，回 main 也改 note.txt
git("checkout", "-b", "feature/conflict")
write("note.txt", "line 1\nBRANCH change\nline 3\n")
git("add", ".")
git("commit", "-m", "feat: branch edits note")

git("checkout", "main")
write("note.txt", "line 1\nMAIN change\nline 3\n")
git("add", ".")
git("commit", "-m", "feat: main edits note")

// 4) 工作樹狀態：staged 修改、unstaged 修改、untracked
write("src/app.ts", "export const greet = (name: string) => `hi ${name}`\n")
git("add", "src/app.ts") // staged 修改

write("README.md", "# git-demo\n\nYuzora M2 GUI acceptance fixture.\n\nEdited (unstaged).\n") // unstaged 修改

write("scratch.txt", "untracked scratch file\n") // untracked

// 5) 摘要
const status = git("status", "--short")
const branch = git("rev-parse", "--abbrev-ref", "HEAD").trim()
console.log("git-demo fixture ready:")
console.log(`  path:     ${dir}`)
console.log(`  branch:   ${branch}`)
console.log("  worktree state (git status --short):")
for (const l of status.split("\n").filter(Boolean)) console.log(`    ${l}`)
console.log("  conflict:  git merge feature/conflict  → note.txt 真衝突")
