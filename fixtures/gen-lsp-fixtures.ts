// fixtures/gen-lsp-fixtures.ts — bun run fixtures/gen-lsp-fixtures.ts
// 於 fixtures/out/lsp-demo 建立四語言最小專案，供 M3 LSP GUI 手動驗收使用。
// 每個專案含刻意型別錯（diagnostics）、跨檔引用（def/references）與 unused import（code action）。
// 界線：只寫入 fixtures/out/lsp-demo（已被 fixtures/.gitignore 的 out/ 排除），不碰本專案原始碼。
// 冪等：每次先 rm -rf 整個 lsp-demo 目錄再重建。

import { rmSync, mkdirSync, writeFileSync } from "node:fs"
import { join, dirname } from "node:path"

const root = join("fixtures", "out", "lsp-demo")

function write(rel: string, content: string) {
    const path = join(root, rel)
    mkdirSync(dirname(path), { recursive: true })
    writeFileSync(path, content)
}

// ts-app：a.ts export、b.ts import（跨檔 def/references）＋刻意型別錯＋unused import。
const tsA = `export interface User {
    id: number
    name: string
}

export function greet(user: User): string {
    return "hello " + user.name
}

export function add(a: number, b: number): number {
    return a + b
}

// b.ts 匯入但不使用 → unused import，供 quick-fix code action。
export function multiply(a: number, b: number): number {
    return a * b
}
`

const tsB = `import { greet, add, multiply, User } from "./a"

const user: User = { id: 1, name: "yuzora" }

// 刻意型別錯：add 第一參數期待 number，傳入 string。
const wrong = add("1", 2)

const msg = greet(user)

console.log(msg, wrong)
`

const tsConfig = `{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "noUnusedLocals": true,
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["*.ts"]
}
`

// py-app：含刻意型別錯（pyright diagnostics）＋型別化函式（hover）。
const pyMain = `"""py-app — pyright diagnostics 與 hover 驗收用最小專案。"""


def add(a: int, b: int) -> int:
    return a + b


def greet(name: str) -> str:
    return "hello " + name


# 刻意型別錯：add 第一參數期待 int，傳入 str → pyright reportArgumentType。
result: int = add("1", 2)

print(greet("yuzora"), result)
`

// rs-app：合法 Cargo package，main.rs 跨檔引用 math.rs＋刻意型別錯（rust-analyzer 可載入）。
const rsCargo = `[package]
name = "rs-demo"
version = "0.1.0"
edition = "2021"
`

const rsMain = `mod math;

fn main() {
    // 刻意型別錯：math::add 期待 i64，傳入 &str → rust-analyzer mismatched types。
    let wrong = math::add("1", 2);
    let msg = math::greet("yuzora");
    println!("{msg} {wrong}");
}
`

const rsMath = `pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

pub fn greet(name: &str) -> String {
    format!("hello {name}")
}
`

// md-doc：markdown preview 驗收用，涵蓋 heading/list/code block/link/blockquote。
const mdReadme = `# yuzora M3 · Markdown Preview Fixture

Markdown preview 驗收用範例，涵蓋 heading、list、code block、link、blockquote 等元素。

## 無序清單

- 第一項
- 第二項
  - 巢狀子項
- 第三項

## 有序清單

1. 步驟一
2. 步驟二
3. 步驟三

## 程式碼區塊

行內程式碼 \`const answer = 42\`，以及區塊：

\`\`\`ts
export function greet(name: string): string {
    return "hello " + name
}
\`\`\`

## 連結與強調

前往 [Tauri 官方文件](https://tauri.app) 了解更多。

**粗體**、*斜體*、~~刪除線~~ 與 \`inline code\`。

> Blockquote：preview 應正確渲染引用區塊與巢狀元素。
`

const files: Record<string, string> = {
    "ts-app/a.ts": tsA,
    "ts-app/b.ts": tsB,
    "ts-app/tsconfig.json": tsConfig,
    "py-app/main.py": pyMain,
    "rs-app/Cargo.toml": rsCargo,
    "rs-app/src/main.rs": rsMain,
    "rs-app/src/math.rs": rsMath,
    "md-doc/README.md": mdReadme
}

// 乾淨重建（冪等）
rmSync(root, { recursive: true, force: true })
for (const [rel, content] of Object.entries(files)) write(rel, content)

// 摘要
console.log("lsp-demo fixture ready:")
console.log(`  path:  ${root}`)
console.log("  apps:")
console.log("    ts-app/   a.ts export＋b.ts import（跨檔）＋型別錯＋unused import multiply")
console.log("    py-app/   main.py 型別錯＋型別化函式")
console.log("    rs-app/   Cargo package：src/main.rs 跨檔引用 src/math.rs＋型別錯")
console.log("    md-doc/   README.md heading/list/code/link/blockquote（preview）")
console.log("  intentional diagnostics:")
console.log('    ts-app/b.ts        add("1", 2)         argument type；multiply unused import')
console.log('    py-app/main.py     add("1", 2)         pyright reportArgumentType')
console.log('    rs-app/src/main.rs math::add("1", 2)   mismatched types')
console.log("  files:")
for (const rel of Object.keys(files)) console.log(`    ${rel}`)
