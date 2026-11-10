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
- 對話框、popup、toast 的進場不在 mod 裡做（`transition` 在元素第一次掛上時不播，而 overlay 是「加入就畫、移除就消失」），在 `app/src/yuzora/motion.rs`，由 `src/window.rs` 的 `render_overlays` 呼叫。速度取自同一份 `motion` token（`yuzora_core::tokens::motion()`），不另寫毫秒數。
- 不用 `loop`。`loop`（脈動）不看減少動態效果，mod 端也讀不到這個設定，而且它讓所在的 view 以約 30 fps 持續重畫。`render_tests/motion_tests.rs` 有一個測試擋住任何 `yuzora-*` mod 呼叫 `.loop(`；之後 mod 端若能讀到減少動態效果，再重新評估。`spin`（圈圈 spinner）沿用現況，只在載入中掛上。

## 引擎的限制，決定了怎麼寫

- 只能補間 `opacity`、`width`、`height`、`left`、`top`。尺寸只認絕對 px：百分比、`w_full`、auto 不會動。要動的元素就得算出 px（資源占比條用面板寬度推出軌道寬度）。
- 第一個值直接採用：元素掛上時不播動畫。要「出現」時淡入，得自己做兩段式渲染，第一幀會閃，所以沒做。
- 補間狀態以元素 id 為鍵：要動的 `div` 一定要 `.id(...)`，而且 id 要跟「這個元素是誰」綁在一起（占比條用 `share-<category>`，因為列會依記憶體排序換位置）。元素被卸載後狀態就丟掉，重新掛上是直接採用新值。
- 沒有顏色補間、沒有 transform、hover 樣式也補不到。所以背景色的切換（選取、hover）仍是瞬間的。
- GPUI 的 cached view 只在位置或內容變了時才重畫，並且重放當初畫下的 primitive，連同當初的不透明度。排版以整數 px 為單位，位移每幀不一定變，不透明度卻每幀都變：只在外層套 `opacity`，mod 畫的東西會停在第一次畫下的淡入程度（測試抓到過：按鈕停在 98%）。所以進場期間 overlay 的 mod view 每一幀都被通知重畫（原生 `transition` 也是這樣做的）。

## 現有的動畫

| 位置 | 觸發 | 看到什麼 | 速度 |
| --- | --- | --- | --- |
| 右側 dock 的模式列（`yuzora-modebar`） | 切到另一個模式 | 選取指示條滑到新位置（開關面板時不滑，直接出現或消失） | `base` |
| 資源用量 popover（`yuzora-perf`） | 每 2 秒一次取樣、或切換排序 | 各類別的占比條平滑變到新的占比 | `base` |
| 設定頁的開關（`yuzora-settings-*/ui.js`，目前只有 General 用） | 按開關 | 旋鈕滑到另一端 | `fast` |
| 對話框與 popup（設定、歡迎畫面、diff、選單、picker、指令面板；`src/window.rs`） | overlay 加入時，一次 | 由透明淡入，並從上方 6 px 落到定位；對話框後面的暗層一起淡入 | `fast` |
| toast（`src/window.rs`） | toast 加入時，一次 | 由透明淡入，並從下方 6 px 升到定位 | `base` |

進場只在 overlay「加入」時播一次：重新 render、內容更新、視窗大小改變、焦點切換都不會重播；同一個 id 關掉再開會再播；系統要求減少動態效果時直接是終態。動畫期間 overlay 立即可互動，位移是 layout 的位移（不是只在 paint 時偏移），所以點擊落在看到的位置上。結束後回到與沒有動畫時完全相同的元素樹，也不再要求任何 frame。關閉仍是瞬間。

不做進場的 overlay：`window`（dock 的抽屜、feedback：放好就留著，開啟時也不該每次重播）與以 `layout` 組成的 overlay（目前只有終端機 peek：裡面是終端機 view，由它自己的 cache 重畫）。

## 驗證

`app/src/mods/render_tests/motion_tests.rs`：把真的 mod 掛進一個視窗，用 `advance_clock` 移動時鐘，從 `painted_quads()` 讀每一幀畫出的背景位置（不是讀描述字串，描述只寫目標）。每個動畫斷言 t=0（還在原位）、中點（ease-out 的 160 ms 走到一半是 87.5%）、終點，以及：

- 補間期間 script render 次數不增加（動畫不進 JS）；
- 到達後 `simulate_next_frame` 回報 0 個待處理的 frame（閒置成本為零）；
- 系統要求減少動態效果時直接到終態。

overlay 的進場在 `app/src/window/overlay_motion_tests.rs`（`window::tests`，每個測試各跑在自己的行程）：在 `advance_clock` 移動的時鐘上，從 `painted_quads()` 讀每一幀。第一幀什麼都沒有、中點是 ease-out 的 87.5%（包含 mod 畫在 cached view 裡的東西與對話框的暗層）、結束後整幀的每個 quad 與沒有動畫時的位置與顏色完全相同、之後不再要求 frame；只播一次與重新加入再播；減少動態效果；第一幀的點擊落在看到的位置、第一幀的鍵盤輸入不遺失；下面的終端機 pane 不重畫、尺寸不變。相位計算是純函式（`yuzora/motion.rs` 的單元測試，含 t=0、t=duration、duration=0、時鐘倒退）。

## 上游檔的改動

`app/src/window.rs`，每處都以 `// yuzora(motion):` 註記（約 16 行）：`OverlayKind` 改成 `pub(crate)`（`yuzora::motion` 要讀它）；`Overlay` 多一個欄位 `entered`（加入的時間，在 `open_overlay` 的 struct 裡設定；以 `cx.background_executor().now()` 取，測試才能移動時鐘）；`render_overlays` 多收 `window`（要求下一個 frame），並在 `body` 完成後、`match o.kind` 之前算相位並套用，對話框暗層的 alpha 乘上相位；呼叫端多傳 `window`。其餘邏輯都在 `yuzora/motion.rs`。同步上游時若 `render_overlays` 或 `Overlay` 被改動，看這幾處。

## 之後

- 退場動畫這一版不做：overlay 現在是移除即消失，退場要延後移除並處理焦點還原。
- 以 `layout` 組成的 overlay 與 `window` 若要進場，要先列出裡面所有的 cached view（terminal、mod）並逐一通知重畫。
