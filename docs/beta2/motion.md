# 動畫（motion）

Yuzora 的 UI 動畫是克制的功能性動畫：幫使用者看出「什麼變了」，不是裝飾。這份文件記規則、現有的動畫、怎麼驗證，以及刻意不做的東西。架構規則見 [architecture.md](architecture.md)，lane 流程見 [lanes.md](lanes.md)。

## 規則

- 一個動畫最長 180 ms，進場與位移用 ease-out。沒有彈簧、沒有彈跳。
- 速度只有兩個名字，寫在 `app/defaults/yuzora/tokens.json` 的 `motion`：`fast`（120 ms，小元素，例如開關的旋鈕）與 `base`（160 ms，位移較大的，例如選取指示條、占比條）。mod 不寫死毫秒數。
- mod 用 `yuzora:core` 的 `motion(name)` 取得 `{ duration, easing }`，直接交給 `el.transition(property, policy)`：

  ```js
  import { motion } from "yuzora:core";
  div().id("knob").absolute().left(on ? 18 : 2).transition("left", motion("fast"));
  ```

  名字不存在會在寫的地方丟錯，不會用某個預設長度默默動起來。`app/crates/yuzora-core/src/tokens.rs` 的測試守住「每個速度 ≤ 180 ms、easing 是 `transition` 認得的四種」。
- 只用 `transition`。它在原生端執行（補間時 script 不重新 render，靜止後不再要求任何 frame），並且**跟隨系統的「減少動態效果」**：為真時狀態直接到終態。
- 不用 `loop`。`loop`（脈動）不看減少動態效果，mod 端也讀不到這個設定，而且它讓所在的 view 以約 30 fps 持續重畫。`render_tests/motion_tests.rs` 有一個測試擋住任何 `yuzora-*` mod 呼叫 `.loop(`；之後 mod 端若能讀到減少動態效果，再重新評估。`spin`（圈圈 spinner）沿用現況，只在載入中掛上。

## 引擎的限制，決定了怎麼寫

- 只能補間 `opacity`、`width`、`height`、`left`、`top`。尺寸只認絕對 px：百分比、`w_full`、auto 不會動。要動的元素就得算出 px（資源占比條用面板寬度推出軌道寬度）。
- 第一個值直接採用：元素掛上時不播動畫。要「出現」時淡入，得自己做兩段式渲染，第一幀會閃，所以沒做。
- 補間狀態以元素 id 為鍵：要動的 `div` 一定要 `.id(...)`，而且 id 要跟「這個元素是誰」綁在一起（占比條用 `share-<category>`，因為列會依記憶體排序換位置）。元素被卸載後狀態就丟掉，重新掛上是直接採用新值。
- 沒有顏色補間、沒有 transform、hover 樣式也補不到。所以背景色的切換（選取、hover）仍是瞬間的。

## 現有的動畫

| 位置 | 觸發 | 看到什麼 | 速度 |
| --- | --- | --- | --- |
| 右側 dock 的模式列（`yuzora-modebar`） | 切到另一個模式 | 選取指示條滑到新位置（開關面板時不滑，直接出現或消失） | `base` |
| 資源用量 popover（`yuzora-perf`） | 每 2 秒一次取樣、或切換排序 | 各類別的占比條平滑變到新的占比 | `base` |
| 設定頁的開關（`yuzora-settings-*/ui.js`，目前只有 General 用） | 按開關 | 旋鈕滑到另一端 | `fast` |

## 驗證

`app/src/mods/render_tests/motion_tests.rs`：把真的 mod 掛進一個視窗，用 `advance_clock` 移動時鐘，從 `painted_quads()` 讀每一幀畫出的背景位置（不是讀描述字串，描述只寫目標）。每個動畫斷言 t=0（還在原位）、中點（ease-out 的 160 ms 走到一半是 87.5%）、終點，以及：

- 補間期間 script render 次數不增加（動畫不進 JS）；
- 到達後 `simulate_next_frame` 回報 0 個待處理的 frame（閒置成本為零）；
- 系統要求減少動態效果時直接到終態。

## 之後

- 對話框、popup、toast 的進場要改 `src/window.rs`（上游檔），是下一個單位。
- 退場動畫這一版不做：overlay 現在是移除即消失，退場要延後移除並處理焦點還原。
