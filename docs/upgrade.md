# 全面依賴升級 Runbook

> 最後驗證：2026-07-11。這份文件記錄 Yuzora 進行跨 Bun、TypeScript、Tauri/Rust 全面升級時的判斷順序、相容策略與驗證 gate。版本號是當次快照；後續執行時仍應重新查詢 registry，不可把本文件中的版本視為永久最新版。

## 1. 目標與範圍

全面升級的目標不是單純讓 manifest 顯示較大的版本號，而是同時滿足：

1. Active direct dependencies 已升到最新 stable，或有明確、可驗證的 compatibility exception。
2. Manifest 與 lockfile 一致，乾淨環境可重現安裝。
3. Breaking API migrations 已完成，不以跳過 typecheck、lint 或 tests 掩蓋問題。
4. 既有 dirty worktree 與無關程式碼不被清理或重寫。
5. 最終能分辨「升級 regression」與「升級前已存在的 baseline 問題」。

目前納入升級判斷的 active manifests：

| 區域 | Manifest | Lockfile |
|---|---|---|
| Root app | `package.json` | `bun.lock` |
| Tauri/Rust | `src-tauri/Cargo.toml` | `src-tauri/Cargo.lock` |
| Remotion site | `site-remotion/package.json` | `site-remotion/bun.lock` |
| CodeMirror performance spike | `spikes/cm6-perf/package.json` | `spikes/cm6-perf/bun.lock` |
| CodeMirror LSP spike | `spikes/cm6-lsp/package.json` | `spikes/cm6-lsp/bun.lock` |

除非另有要求，不把下列內容當成 active upgrade target：

- `.superpowers/sdd/snapshots/**` 等歷史快照。
- 無 dependency 的輸出 fixture。
- 由 package manager 管理、不能獨立選版的 transitive dependency。

Transitive dependencies 仍須更新到父套件 constraints 允許的最新解析結果，但不能為了追求表面上的「全部 latest」強制破壞父套件的相容範圍。

## 2. 升級前先建立 baseline

### 2.1 讀取專案規則

先讀 `CLAUDE.md` 與使用者提供的 `AGENTS.md`。本專案的重要限制：

- JavaScript／TypeScript 使用 Bun。
- Python 指令使用 `uv run python`。
- 未經使用者明確下令，不執行 `git add`、`commit` 或 `push`。
- 保留 dirty worktree；不要用 reset、checkout 或 clean 清掉既有修改。

### 2.2 記錄工作樹

```bash
git status --short --branch
git diff --stat
git ls-files --others --exclude-standard
```

如果預計修改的 manifest／lockfile 原本已是 dirty，先保存其 diff 或 hash。後續 review 必須能區分原有修改與本次升級。

### 2.3 跑升級前驗證

```bash
bun run typecheck
bun run build
bun run lint
bun run test
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

失敗項目要記錄完整 test name、error 與數量。升級後只有新出現或因升級擴大的錯誤才是 migration blocker；不要順手重構無關的既存問題。

## 3. 完整版本盤點

### 3.1 找出 active manifests

```bash
rg --files \
  -g 'package.json' \
  -g 'bun.lock*' \
  -g 'Cargo.toml' \
  -g 'Cargo.lock' \
  -g 'tsconfig*.json'
```

人工排除 snapshots、build output 與 fixture，再對每個 active Bun project 執行：

```bash
bun outdated --no-cache --frozen-lockfile --no-progress
bun pm ls
bun install --frozen-lockfile --dry-run --ignore-scripts --no-progress
```

Rust 使用：

```bash
cargo metadata --manifest-path src-tauri/Cargo.toml \
  --format-version 1 --locked

cargo update --manifest-path src-tauri/Cargo.toml \
  --dry-run --verbose
```

`cargo update --dry-run` 只回答目前 semver constraints 內可更新什麼。若要判斷 major upgrade，還必須查 crates.io 最新 non-yanked stable 版本與 upstream changelog。

### 3.2 版本狀態分類

每個 direct dependency 至少標記成下列一類：

| 類型 | 判斷 | 動作 |
|---|---|---|
| Lock-only patch | Manifest range 已涵蓋 latest | 更新 lockfile後直接驗證 |
| Manifest patch/minor | Current range或 lower bound落後 | 更新 manifest 與 lockfile |
| Breaking major | Latest 不在目前 range | 先讀 migration guide，再獨立成一波 |
| Exact pin | `Update == Current` 但 `Latest` 較新 | 確認 pin 原因後修改 exact version |
| Compatibility exception | 舊版仍是其他工具的必要 API provider | 保留並記錄原因；另裝 latest executable |
| Unused direct dependency | Source code沒有直接使用 | 移除 direct declaration，不必升一個未使用的套件 |

預發布版本不算 stable latest。例如 `-rc`、`-beta`、`-dev`、`-alpha` 應另外列出，不要在一般全面升級中自動採用。

## 4. 建議分波順序

不要一次修改所有 API 後才首次編譯。建議順序：

1. **Bun lock-only／patch dependencies**：更新後先跑 root typecheck、lint、build。
2. **TypeScript compiler**：獨立處理 TS defaults、compiler API consumers 與所有 `tsconfig`。
3. **子專案**：Remotion 與 spikes 各自安裝、typecheck／lint。
4. **Rust lock-only patches**：先確定 Cargo baseline仍綠。
5. **Rust breaking majors**：簡單 API、watcher、HTTP client 分開處理。
6. **整合 gate**：完整 JS tests、Rust tests、frozen lockfile與 outdated audit。

若兩個工作項目會修改同一個 manifest、lockfile或共用 config，應依序處理，不要並行寫入。

## 5. TypeScript 7 升級策略

### 5.1 為何需要 side-by-side

TypeScript 7.0 是新的 native compiler，但 7.0 不提供舊版 compiler API。`typescript-eslint@8.63.0` 的支援範圍仍是 TypeScript `<6.1.0`。因此本專案採：

- `@typescript/native`：TypeScript 7，實際負責 build/typecheck。
- `typescript`：TypeScript 6，只提供 `typescript-eslint` 需要的 compiler API。

Root 參考配置：

```json
{
  "devDependencies": {
    "@typescript/native": "npm:typescript@7.0.2",
    "typescript": "~6.0.3"
  },
  "scripts": {
    "typecheck": "bun ./node_modules/@typescript/native/bin/tsc --noEmit -p tsconfig.json && bun ./node_modules/@typescript/native/bin/tsc --noEmit -p tsconfig.node.json",
    "build": "bun run typecheck && vite build"
  }
}
```

不要假設 `.bin/tsc` 的 alias collision順序。Scripts 要顯式執行 `@typescript/native/bin/tsc`。

官方建議的 `typescript: npm:@typescript/typescript6` wrapper 在 Bun 1.3.14 曾出現 package resolution錯誤；再次採用前必須在當時 Bun 版本重新驗證。

參考：

- [TypeScript 7.0 release與 side-by-side說明](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/)
- [typescript-eslint dependency support](https://typescript-eslint.io/users/dependency-versions/)

### 5.2 TypeScript 7 defaults檢查

TypeScript 7 將 `types` 預設為 `[]`。過去由某個 `@types/*` package 間接帶入的 globals或 lib可能消失，因此要顯式檢查：

- `types` 是否列出 Node、Bun、Vitest等真正需要的 globals。
- `lib` 是否與 `target` 一致。
- 每一份 referenced `tsconfig` 是否真的有被 typecheck。
- `@ts-expect-error` 是否已變成 unused suppression。

這次 `site-remotion` 的 `target` 是 ES2018、`lib` 卻只有 ES2015；TS 7 因而正確指出 `Array.prototype.includes` 不存在。修正方式是把 `lib` 對齊 ES2018，而不是重新依賴隱含的 `@types/node`。

`tsconfig.node.json` 是 composite project，typecheck可能產生 `.tsbuildinfo`。將它寫到已忽略的位置：

```json
{
  "compilerOptions": {
    "tsBuildInfoFile": "./node_modules/.cache/tsconfig.node.tsbuildinfo"
  }
}
```

### 5.3 驗證實際 compiler

```bash
bun ./node_modules/@typescript/native/bin/tsc --version

cd site-remotion
bun ./node_modules/@typescript/native/bin/tsc --version
```

兩者應顯示目前選定的 TS 7 stable，而不是 compatibility TS 6。

## 6. ESLint 10 相容處理

這次 root 在 `eslint@10.6/10.7 + typescript-eslint@8.63.0` 曾因 browser globals觸發：

```text
TypeError: scopeManager.addGlobals is not a function
```

本專案所有 `src` files都是 TypeScript，`typescript-eslint` 官方也不建議在 TS files使用 core `no-undef`。因此 root config採：

- 移除 `globals.browser`。
- 對 `src/**/*.{ts,tsx}` 關閉 core `no-undef`。
- 移除不再使用的 direct `globals` dependency。
- Root lint忽略有獨立 config／lockfile 的 `site-remotion`。

不要為了消除既存 React compiler warnings而在 dependency upgrade中重構 components。Warnings 與 errors 要分開記錄。

`@remotion/eslint-config-flat@4.0.487` 內部固定舊 `typescript-eslint`，與 ESLint 10／TS 7 不相容。Remotion project採用：

```json
{
  "devDependencies": {
    "@typescript/native": "npm:typescript@7.0.2",
    "typescript": "6.0.3"
  },
  "overrides": {
    "typescript-eslint": "8.63.0"
  }
}
```

未來 Remotion config更新內部 dependency後，應重新測試並考慮移除 override。

## 7. Bun／CodeMirror lockfile處理

一般更新流程：

```bash
bun install --no-progress
bun outdated --no-cache --frozen-lockfile --no-progress
bun install --frozen-lockfile --dry-run --ignore-scripts --no-progress
```

如果 direct CodeMirror升級後出現下列型別錯誤：

```text
Types have separate declarations of a private property 'flags'
```

代表 lockfile同時解析多份 `@codemirror/state`／`@codemirror/view`。這不是用 type assertion掩蓋的問題；應統一 dependency graph：

```json
{
  "overrides": {
    "@codemirror/state": "6.7.1",
    "@codemirror/view": "6.43.6"
  }
}
```

接著執行：

```bash
bun install --force --no-progress
bunx tsc --noEmit -p tsconfig.json
```

版本號每次都要重新查詢，不要永久照抄上面的 2026-07-11 快照。

## 8. Rust breaking upgrade playbook

2026-07-11 這次升級的主要 direct crates：

| Crate | Before | After | Migration重點 |
|---|---:|---:|---|
| `dirs` | 5.0.1 | 6.0.0 | 現有 `home_dir()` API仍可用 |
| `notify` | 6.1.1 | 8.2.0 | 與 debouncer同波；驗證跨平台 watcher |
| `notify-debouncer-mini` | 0.4.1 | 0.7.0 | 強制與 notify major對齊 |
| `rand` | 0.8.6 | 0.10.2 | 驗證 askpass token／socket命名路徑 |
| `ureq` | 2.12.1 | 3.3.0 | HTTP API rewrite，需改 source |
| `sha2` | 0.10.9 | 0.11.0 | 驗證 checksum vectors |
| `zip` | 6.0.0 | 8.6.0 | 驗證 log export／解壓縮 |
| `sysinfo` | 0.39.5 | 0.39.6 | Lock／patch upgrade |
| `webpki-roots` | 0.26.11 | 1.0.8 | 驗證 database TLS建構 |
| `ignore` | 0.4.27 | 0.4.28 | Lock／patch upgrade |

`thiserror` 沒有被 Yuzora source直接使用，因此移除 direct declaration；不為未使用套件保留一個表面上的最新版本。

### 8.1 ureq 2 → 3 API對照

| ureq 2 | ureq 3 |
|---|---|
| `ureq::AgentBuilder::new()` | `ureq::Agent::config_builder()` |
| `timeout_connect(duration)` | `timeout_connect(Some(duration))` |
| `timeout_read(duration)` | `timeout_recv_response(Some(duration))` + `timeout_recv_body(Some(duration))` |
| `response.header(name)` | `response.headers().get(name)` |
| `response.into_reader()` | `response.body_mut().as_reader()` 或 owned body reader |
| `Error::Status(status, response)` | 預設為 `Error::StatusCode`; 要保留 error body時設 `http_status_as_error(false)` |

升級 HTTP client時必須保留原有安全邊界：

- Connect timeout。
- Response／body timeout。
- Declared `Content-Length` pre-check。
- Chunked／unknown length的 streaming size cap。
- HTTP error body讀取行為。

### 8.2 Rust更新與驗證

```bash
cargo update --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo update --manifest-path src-tauri/Cargo.toml --dry-run --verbose
```

最後一個 dry run應顯示 `Locking 0 packages`。仍被列為 available的 transitive crates，需要用 `cargo tree --invert <crate>@<version>` 確認是哪個父套件限制，不要任意 force incompatible resolution。

## 9. 最終驗證 Gate

### Root app

```bash
bun run typecheck
bun run lint
bun run build
bun run test
```

### 子專案

```bash
cd site-remotion
bun run lint

cd ../spikes/cm6-perf
bunx tsc --noEmit -p tsconfig.json

cd ../cm6-lsp
bunx tsc --noEmit -p tsconfig.json
```

### Rust

```bash
cargo check --manifest-path src-tauri/Cargo.toml --quiet
cargo test --manifest-path src-tauri/Cargo.toml --quiet
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

### Lockfile與版本

對四個 Bun projects重跑：

```bash
bun outdated --no-cache --frozen-lockfile --no-progress
bun install --frozen-lockfile --dry-run --ignore-scripts --no-progress
```

並確認沒有生成檔留在 repository root：

```bash
test ! -e vite.config.js
test ! -e vite.config.d.ts
test ! -e tsconfig.node.tsbuildinfo
git diff --check
git status --short
```

## 10. 失敗判讀規則

1. **先重跑 focused test**：如果 full suite只失敗一個 timing-sensitive test，先用 `--exact` 或相同 filter連跑三次。
2. **再重跑 full suite**：focused test穩定通過後，完整 suite仍須再綠一次。
3. **不得只以重跑掩蓋 deterministic failure**：同一 assertion持續失敗就是 blocker。
4. **Baseline error不自動擴大 scope**：記錄但不順手修，除非它妨礙判斷本次升級是否成功。
5. **Upgrade-induced error必須解決**：例如 duplicate CodeMirror types、removed ureq API、TS 7 config defaults。

本次 Rust full suite第一次曾在 `agent_process::tests::exit_callback_receives_stderr_tail` 出現一次 timing failure；focused test連跑三次與完整 suite重跑均通過。後續若重複出現，應獨立追蹤 flaky test，不要歸因於 dependency upgrade。

## 11. 2026-07-11 完成快照

本次完成後的主要驗證結果：

| Gate | 結果 |
|---|---|
| Root TypeScript compiler | 7.0.2 |
| Remotion TypeScript compiler | 7.0.2 |
| Root build | 通過，Vite 8.1.4 |
| Root lint | 通過；0 errors、48 existing warnings |
| Root tests | 112 files、1374 tests通過 |
| Cargo check | 通過 |
| Cargo tests | 312 tests通過 |
| Cargo compatible updates | 0 |
| `cm6-perf` typecheck | 通過 |
| `cm6-lsp` typecheck | 仍有升級前既存 3 errors；無 TS 7新增 regression |

`bun outdated` 在 root與 Remotion仍會列出 `typescript@6.0.3 → 7.0.2`。這是刻意保留的 compiler API compatibility dependency，不代表 build仍使用 TS 6。判斷時必須同時驗證 `@typescript/native/bin/tsc --version`。

## 12. 升級完成檢查表

- [ ] 已確認 active manifests與排除項目。
- [ ] 已記錄升級前 dirty tree與 baseline failures。
- [ ] 所有 direct dependencies已是 latest stable，或有書面 compatibility exception。
- [ ] Exact pins已逐一確認與更新。
- [ ] Unused direct dependencies已移除。
- [ ] Manifest／lockfile通過 frozen install。
- [ ] TypeScript實際 compiler版本已驗證。
- [ ] 每份 `tsconfig` 都有被 typecheck。
- [ ] ESLint完成且 errors為零。
- [ ] Root build與完整 tests通過。
- [ ] Cargo check、fmt、完整 tests通過。
- [ ] `cargo update --dry-run` 沒有 compatible updates。
- [ ] 沒有意外生成檔或無關 diff。
- [ ] Compatibility exceptions與既存 warnings/errors已記錄。
- [ ] 未經明確授權，沒有 commit或push。
