# IntelliJ IDEA Community：Git 整合功能與效能架構調查

調查日期：2026-11-02。來源是 `JetBrains/intellij-community` master，commit `5935c1f8`。用 blobless sparse clone 靜態讀碼，沒有執行任何程式。調查者：scout subagent，由總指揮 session 整理成檔。

## 摘要

1. **原始碼位置**：Git 專屬功能在 `plugins/git4idea/{backend,shared,frontend,rpc.backend,hosting}`；通用 VCS 平台在 `platform/{vcs-impl,vcs-log,dvcs-impl,diff-impl,lvcs-impl}`。
2. **執行方式**：不用 libgit2，也沒有自製協定，所有操作都是 spawn `git` CLI。
3. **status**：只靠 `git status --porcelain -z` 加上 dirty scope 增量刷新，不用 fsmonitor。
4. **log 分兩階段讀取**：
   - 第一階段讀前 2×1000 筆完整 metadata。
   - 第二階段用 `--date-order` 讀全部 commit 的 `hash+parents+time`，建出 permanent graph。
   - commit details（變更檔案）延遲到需要時才讀。
5. **persistent index 預設關閉**：master 的 `registry.properties` 是 `vcs.log.index.enable=false`（commit `e55bc60dfa`，2023-09-05）。關閉時，user／text／path filter 改走 `git log`，並分階段提高 `--max-count`（5、100、2000、全部）。
6. **index 後端**：預設用 PersistentHashMap；SQLite（FTS5 trigram）後端要另外開 registry 才會用。兩者都存在 `<systemPath>/vcs-log/`。
7. **graph**：用 int 陣列壓縮的 PermanentLinearGraph，單一 parent 而且接在下一列的節點不存邊。visible graph 由 controller 鏈即時衍生，print element 逐 row 產生並用 SLRU cache 快取。
8. **指令層**：
   - 只有 WRITE 指令會取專案級寫鎖。
   - 讀指令設 `GIT_OPTIONAL_LOCKS=0`。
   - 取消靠每 50ms 輪詢 `checkCanceled`。
   - pathspec 超過 7600 字元就退回掃整個 root。
9. **diff**：文字 diff 全部由 IDE 自己計算（Myers O(ND)，過大時退回 Patience）。`git diff` 只用在 `--name-status`／rename 偵測，檔案內容用 `git cat-file` 取得。
10. **查不到**（下文標為「未找到」）：commit-graph／multi-pack-index 設定、sparse-checkout 與 partial clone（`--filter`）UI、hooks 編輯 UI、`git log -L`、git 端預設啟用 fsmonitor。

---

## A. 使用者可見功能清單

**路徑縮寫**：以下皆相對於 `/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/beta2-research/intellij`。

| 縮寫 | 路徑 |
| --- | --- |
| `G/` | `plugins/git4idea/backend/src/` |
| `GSH/` | `plugins/git4idea/shared/src/git4idea/` |
| `GX` | `plugins/git4idea/backend/resources/intellij.vcs.git.backend.xml` |
| `VI/` | `platform/vcs-impl/src/com/intellij/openapi/vcs/` |
| `VA` | `platform/vcs-impl/resources/META-INF/VcsActions.xml` |
| `VL/` | `platform/vcs-log/impl/src/com/intellij/vcs/log/` |
| `VG/` | `platform/vcs-log/graph/src/com/intellij/vcs/log/graph/` |
| `DI/` | `platform/diff-impl/src/com/intellij/diff/` |
| `DV/` | `platform/dvcs-impl/src/com/intellij/dvcs/` |
| `UD/` | `platform/util/diff/src/com/intellij/` |
| `REG` | `platform/util/resources/misc/registry.properties`（本機副本在 `/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/beta2-research/raw/registry.properties`） |

**標籤**：`[Git]` 是 git 專屬，`[VCS]` 是 VCS 平台通用，`[DVCS]` 是 git 與 hg 共用。

### A1. Clone、init、環境、多 root

- **Clone**：支援 URL、shallow clone（depth）、unshallow；預設 recurse submodules（advanced `git.clone.recurse.submodules=true`，`GX:666`）。`[Git]`＋`[DVCS]`
  - 路徑：`G/checkout/GitCheckoutProvider.java`、`G/ui/GitShallowCloneViewModel.kt`、`G/actions/GitUnshallowRepositoryAction.kt`、`DV/ui/CloneDvcsDialog.java`
- **Init**：`Git.Init`。`[Git]`
  - 路徑：`G/repo/GitRepositoryInitializer*.java`、`G/roots/GitIntegrationEnabler.java`
- **Git executable**：自動偵測、下載、WSL／Eel、版本 gate（`GitVersionSpecialty`）。`[Git]`
  - 路徑：`G/config/GitExecutableDetector.java`、`GitDownloadAndInstall.kt`、`GitExecutableManager.java`、`GitVersionSpecialty.kt`
- **VCS root 自動偵測**：`[VCS]`
  - registry：`vcs.detect.vcs.mappings.automatically`、`vcs.root.detector.folder.depth=-1`、`vcs.root.detector.ignore.pattern=[.].*|vendor`（`REG:777-786`）
  - Git 端：`G/roots/GitRootChecker.java`
- **Multi-root**：`DvcsSyncSettings` 讓多個 repo 同步執行分支操作；另有 `GitMultiRootBranchConfig`，log 左側以色條區分 root。`[DVCS]`
  - 路徑：`DV/branch/DvcsMultiRootBranchConfig.java`、`G/ui/branch/GitMultiRootBranchConfig.java`
- **Submodules**：解析 `.gitmodules`，把 submodule 當成獨立 repo；status 時處理父子關係；update 時跑 `git submodule`。`[Git]`
  - 路徑：`G/repo/GitModulesFileReader.kt`、`G/repo/GitSubmodule.kt`、`G/update/GitSubmoduleUpdater.kt`、`G/status/GitChangeProvider.java:62-70`
- **Safe mode**：專案未受信任時不執行 git，丟 `GitCommandNotTrustedException`（`G/commands/GitHandler.java:479-488`）。`[Git]`

### A2. Status、Changes view、Changelists、Partial commit

- **Changes view**：Local Changes、Unversioned、Ignored、Conflicts 節點。`[VCS]`，Git 端由 provider 提供資料。
  - 路徑：`VI/changes/ChangesViewManager.kt`、`G/status/GitChangeProvider.java`、`GitChangesCollector.java`
- **Changelists**：新增、改名、刪除、設為 active、搬移、silently shelve；狀態存在 workspace.xml。`[VCS]`
  - 路徑：`VI/changes/ChangeListManagerImpl.kt`（`@State ChangeListManager`，`:95`）、`ChangeListWorker.java`
  - action（在 `VA`）：`ChangesView.NewChangeList`、`Rename`、`RemoveChangeList`、`SetDefault`、`Move`
  - 整個功能可用 registry `vcs.disable.changelists` 停用（`REG:800`）
- **Partial commit（chunk／line 級）**：
  - Changelist 模式下，`PartialLocalLineStatusTracker` 追蹤每個 range 屬於哪個 changelist。`[VCS]`
  - 相關 action：`Vcs.RollbackChangedLines`、`Vcs.MoveChangedLinesToChangelist`、`Vcs.Diff.IncludeChangedLinesIntoCommit`
  - 路徑：`VI/ex/PartialLocalLineStatusTracker.kt`、`VI/impl/LineStatusTrackerManager.kt`、`PartialChangesUtil.kt`、`platform/vcs-impl/src/com/intellij/vcs/commit/PartialCommitInclusionModel.kt`
  - Git 端提交時，把「選中行的合成內容」寫成 index blob：`G/checkin/GitCheckinEnvironment.kt:437-497`。`[Git]`
- **Staging area 模式**（取代 changelists）：`[Git]`
  - 設定說明寫明開啟後會停用 changelists（`plugins/git4idea/shared/resources/messages/GitBundle.properties:842-843`）。
  - 功能：stage 檔案／chunk／line，並把 index 檔當成虛擬檔案編輯。
  - 另有 HEAD／Staged／Local 三版比較、Stage 內的 accept yours／theirs／merge、stage stash。
  - 路徑：`G/index/*`（`GitStageTracker`、`GitStageLineStatusTracker`、`vfs/GitIndexVirtualFile.kt`、`actions/Git*Action.kt`）；action 在 `GX` 的 `Git.Stage.*`
- **Commit tool window**：預設是非 modal 的 commit tool window（`platform/vcs-impl/src/com/intellij/vcs/commit/*`）。`[VCS]` Modal commit 已拆成獨立外掛 `plugins/git-modal-commit`，`GitModalCommitDeprecationNotifier` 會提示它已棄用。`[Git]`
- **Ignored files**：`[Git]`
  - 可編輯 `.gitignore` 與 `.git/info/exclude`，提供 Ignore file action，可把 ignored 標記成 excluded。
  - 路徑：`G/ignore/*`（`GitIgnoreChecker`、`actions/GitIgnoreFileActionGroup`、`lang/GitIgnoreFileType`）；action 為 `GX` 的 `Git.OpenExcludeFile`、`Git.Ignore.File`
  - 設定：`VcsConfiguration.MARK_IGNORED_AS_EXCLUDED`（`G/config/GitVcsPanel.kt:105`）

### A3. Commit 選項

- **`git commit` 參數**，由 `commitStaged` 組出（`G/checkin/GitRepositoryCommitter.kt:119-130`）。`[Git]`
  - amend 最後一筆：`--amend`
  - amend 指定 commit：先 `--allow-empty` 建立，再事後 squash（`GitAmendSpecificCommitSquasher.kt`；registry `git.amend.specific.commit=true`）
  - sign-off：`--signoff`
  - 跳過 hooks：`--no-verify`
  - 訊息清理：`--cleanup=strip`
  - 作者與日期：`--author=…`、`--date`
  - rename 分開 commit：持久化選項 `COMMIT_RENAMES_SEPARATELY`
- **GPG**：IDE 不傳 `-S`，而是改 git config（`commit.gpgSign`、`user.signingKey`；`GitBundle.properties:864-866`）。`[Git]`
  - 路徑：`G/config/gpg/*`（`GpgSignConfigurableRow`、`GitGpgConfigDialog`、`GitGpgMultiRootConfigDialog`）、`G/commit/signing/*`（`GpgAgentConfigurator`）
  - 內建 pinentry：registry `git.commit.gpg.signing.enable.embedded.pinentry=true`
  - log 會顯示簽章驗證結果：`G/log/GitCommitSignatureLoader.kt:22-40`、`GitCommitSignatureStatusProvider`
- **Hooks**：由 git 自己執行。`[Git]`
  - 每次 commit 可勾選跳過（`G/checkin/GitSkipHooksCommitHandlerFactory.kt`）。
  - 全域 advanced 設定：`git.commit.do.not.run.commit.hooks=false`（`GX:672`）。
  - 會偵測 hook 是否存在（`G/repo/GitHooksInfo.kt`、`GitRepositoryReader.readHooksInfo`），也讀 config 形式的 hooks（`hook.*`，見 `GitVersionSpecialty.CONFIG_BASED_HOOKS`）。
  - 未找到 hooks 的編輯或管理 UI。
- **Commit message**：
  - 範本（`commit.template`）：`G/repo/GitCommitTemplateTracker.kt`、`G/commit/GitTemplateCommitMessageProvider.kt`
  - merge／rebase 訊息：`GitMergeCommitMessagePolicy`
  - `fixup!`／`squash!`／`amend!` 補全：`GitCommitCompletionContributor`（registry `git.commit.completion.fixup.squash`）
  - 最近 commit 清單上限 20 筆（registry `git.commit.recent.commits.limit=20`）
  - 訊息歷史（`Vcs.ShowMessageHistory`）、Co-Authored-By 補全、subject／body 長度檢查（`platform/vcs-impl/src/com/intellij/vcs/commit/message/*`）。`[VCS]`
- **Pre-commit checks**：
  - Git 端（`GX:806-813` 的 `vcsCheckinHandlerFactory` 清單，實作在 `G/checkin/*`、`G/crlf/*`）：
    - `user.name` 檢查
    - CRLF 警告
    - 大檔警告（預設 50MB）
    - 非法檔名
    - detached HEAD 或 rebase 中 commit 的警告
    - cherry-pick continue
    - 未解決的 merge
  - 平台端：reformat、rearrange、optimize imports、code cleanup、analyze、TODO、external tools（`platform/vcs-impl/lang/src/com/intellij/openapi/vcs/checkin/*`、`VcsExtensions.xml:225-336`）。`[VCS]`
- **Commit and Push**：有設定「Show Push dialog for Commit and Push」，可限定只對 protected branch 顯示（`G/checkin/GitCommitAndPushExecutor.kt`）。`[Git]`

### A4. Shelf 與 Stash

- **Shelf**：`[VCS]`
  - 功能：shelve／unshelve（含 silent、partial、還原已 unshelve 的項目）、import patch、更改存放位置。
  - 預設位置 `<project>/.idea/shelf`（`VI/changes/shelf/ShelveChangesManager.kt:1453-1473`）。
  - 路徑：`VI/changes/shelf/*`；action 在 `VA`：`ChangesView.Shelve*`、`Vcs.Shelf.*`、`ShelvedChanges.*`
  - Git 有一個批次設定，shelve 時預先載入 base revision（registry `git.shelve.load.base.in.batches=1000`）。
- **Stash**：`[Git]`
  - 功能：stash（含 `--keep-index`）、silently、pop、apply、unstash as（開新分支、恢復 index）、drop、clear。
  - 有 Stash tab，diff 可以對 local 或對 parent；也能和 Shelf 合併成同一個 tab。
  - 路徑：`G/stash/*`、`G/ui/GitUnstashDialog.java`；action 在 `GX`：`Git.Stash*`、`Git.Unstash`、`Git.Show.Stash`
  - 設定：`settings.enable.stashes.and.shelves`、`COMPARE_WITH_LOCAL_IN_STASHES_ENABLED`
- **更新或 checkout 時保存本機變更的策略**：`GitSaveChangesPolicy {STASH, SHELVE}`，預設 SHELVE（`GSH/config/GitVcsOptions.kt` 的 `SAVE_CHANGES_POLICY`）。`[Git]`
  - 實作：`G/util/GitPreservingProcess.kt`、`G/stash/GitShelveChangesSaver.java`、`GitStashChangesSaver.java`

### A5. Branches、Tags、Remotes

- **Branch popup、toolbar widget、log 內的 Branches 面板**：`[Git]`
  - 新增分支。
  - checkout，包括 checkout as new branch、checkout and rebase onto current、checkout and update、checkout revision、checkout from input。
  - rename、delete（本地、遠端、tag）、tracked branch 子選單。
  - favorites、recent branches、依目錄或 repo 分組、顯示 tags、incoming／outgoing 指示。
  - 路徑：`G/actions/branch/*`、`G/actions/ref/*`、`G/actions/tag/*`、`G/ui/branch/*`、`G/ui/branch/dashboard/*`、`plugins/git4idea/shared/resources/intellij.vcs.git.shared.xml`、`GX:132-250`
- **Smart checkout 與強制 checkout**：`G/branch/GitBranchWorker.java`、`GitSmartOperationDialog.java`、`GitCheckoutOperation.java`。`[Git]`
  - 分支已在其他 worktree checkout 時的處理：`G/branch/GitCheckoutInOtherWorktreeDialogs.kt`
- **Compare**：`[Git]`
  - 與 branch 或 tag 比較：`Git.CompareWithBranch`（`GitCompareWithRefAction`）
  - 與工作樹比較：`GitShowDiffWithBranchPanel.kt`
  - 兩分支比較：`GitCompareBranchesUi.kt`，可左右對調（`SWAP_SIDES_IN_COMPARE_BRANCHES`）
- **Branch cleanup**：`Git.Cleanup.Branches`、`FindMergedLocalBranchesAction`；清理用的符號由 advanced `git.branch.cleanup.symbol="-"` 設定。`[Git]`
- **Protected branches**：以 regex 清單設定，禁止 force push，可從 GitHub 同步規則（`G/GitProtectedBranches.kt`、`G/config/GitSharedSettings.java`、`GitProtectedBranchProvider.kt`）。`[Git]`
- **Upstream 追蹤**：`GitBranchTrackInfo`、`G/update/FixTrackedBranchDialog.kt`、`G/actions/branch/GitTrackedBranchActionGroup.kt`；push 時可 set-upstream（`GitPushOperation`）。
  - 未找到獨立的「set upstream」action，只看到 rename 與 push 兩條路徑。
- **Tags**：建立（`GitTagDialog`）、push 單一 tag、push 全部 tags（`GitPushTagMode`：`--tags`／`--follow-tags`）、刪除本地或遠端 tag、checkout tag。`[Git]`
  - 路徑：`G/actions/GitCreateTagAction`、`G/actions/tag/*`、`G/branch/GitDeleteTagOperation.java`、`GitDeleteRemoteTagOperation.java`
- **Remotes 管理**：新增、編輯、刪除，記住 HTTP 認證（`G/remote/GitConfigureRemotesDialog.kt`、`GitDefineRemoteDialog.kt`、`GitRememberedInputs.java`）。`[Git]`
- **Fetch、Pull、Update**：
  - fetch tags 模式：Auto／Sync／Always／Never（`GitFetchTagsMode`）
  - Pull dialog 選項：`--rebase`、`--ff-only`、`--no-ff`、`--squash`、`--no-commit`、`--no-verify`（`G/pull/GitPullOption.kt`）
  - Update project：UpdateMethod 有 merge／rebase／reset／branch default 四種；更新後可用 log 顯示結果（registry `git.update.project.info.as.log=true`）
  - 路徑：`G/update/*`（`GitMergeUpdater`、`GitRebaseUpdater`、`GitResetUpdater`、`GitUpdateProcess`）
  - 未找到 `fetch --prune` 的固定設定，只看到 `PRUNE_TAGS`（`--prune-tags`）。
- **Push**：
  - Push dialog 預覽：`DV/push/*`、`G/push/*`
  - force-with-lease：advanced `git.use.push.force.with.lease=true`（`GX:669`；實作在 `G/push/GitPushOperation.java:113,462-466`）
  - 推 tags、跳過 hooks（`--no-verify`）、被拒時自動 update 後重試（設定 `autoUpdateIfPushRejected`）
  - 記住最近的 push target（上限 10）
  - 「Push All up to Here」（`Git.PushUpToCommit`）、「Add Commits to Remote Branch」（`G/actions/addCommit/*`）
  - force push 預設要確認，可用 advanced `vcs.allow.force.push.without.confirmation` 關閉確認（`VcsExtensions.xml:357`）

### A6. Merge、Rebase、Cherry-pick、Revert、Reset、Undo

- **Merge**：`[Git]`
  - 選項：`--no-ff`、`--ff-only`、`--squash`、`-m`、`--no-commit`、`--no-verify`、`--allow-unrelated-histories`（`G/merge/GitMergeOption.kt:11-17`）
  - 動作：Merge Commit、Abort Merge、smart merge
- **Rebase**：
  - dialog 選項：switch branch、`--onto`、`--rebase-merges`、`--keep-empty`、`--root`、`--interactive`、`--update-refs`、`--autosquash`／`--no-autosquash`（`G/rebase/GitRebaseOption.kt:13-21`）
  - Continue、Abort、Skip（`GX` 的 `Git.Rebase.*`）
  - 互動式 rebase 編輯器：
    - entry 動作：pick／edit／drop／reword／squash／fixup／update-ref（`G/rebase/GitRebaseEntry.kt:17-34`）
    - 實作：`G/rebase/interactive/*`、`GitInteractiveRebaseEditorHandler.java`
    - 做法是把 `GIT_EDITOR` 指向 IDE 的 helper（`G/rebase/GitHandlerRebaseEditorManager.java:67`），helper 程式在 `plugins/git4idea/rt/src/git4idea/editor/`。
  - 在 log 裡直接編輯歷史：Reword、Squash Commits、Drop Commits、Fixup、Squash Into、Interactive Rebase from Here、Extract Selected Changes、Drop Selected Changes（`GX:73-120`、`G/rebase/log/*`）
  - rebase 前後的問題偵測：`GitRebaseProblemDetector`、`GitRebaseOverMergeProblem`
- **In-memory 歷史編輯**：pick、reword、fixup、drop 都不動工作樹與 index（`G/inMemory/*`）。`[Git]`
  - 設計受 git-revise 啟發，見 `NOTICE` 與 `LICENSE.git-revise`。
  - registry：`git.in.memory.commit.editing.operations.enabled=true`（`GX:786`）
- **Cherry-pick**（`G/cherrypick/*`）：`[Git]`
  - 預設逐 commit 套用；只有在 `git.cherry.pick.use.git.sequencer` 不是預設的 false 時才用 git sequencer（`GX:790`）。
  - 已發佈的 commit 會加上「(cherry picked from commit …)」後綴（設定 `ADD_SUFFIX_TO_CHERRY_PICKS_OF_PUBLISHED_COMMITS=true`）。
  - 空 commit 的策略：SKIP 或 CREATE（`GitVcsApplicationSettings.EMPTY_CHERRY_PICK_RESOLUTION_STRATEGY`，預設 SKIP）。
  - 支援 Continue、Abort。
  - log 有「Not Cherry-Picked Commits」highlighter，用 `git cherry` 加 index 判斷（`G/branch/CherryPickedCommitsHighlighter.kt`）。
  - 只套用部分檔案或 hunk：`Vcs.ApplySelectedChanges`。`[VCS]`
- **Revert**：`G/revert/GitRevertProcess.kt`、`Git.Revert.In.Log`、Abort Revert；只 revert 選取的變更：`Vcs.RevertSelectedChanges`。
- **Reset**：四種模式 `--soft`、`--mixed`、`--hard`、`--keep`（`GSH/reset/GitResetMode.java:13-16`）；入口有 `Git.Reset`（`GitNewResetDialog`）與 `Git.Reset.In.Log`。`[Git]`
- **Undo commit**：`Git.Uncommit`（`G/reset/GitUncommitAction.java`），用 soft reset 實作並把變更放回 changelist。`[Git]`
- **Rollback 與 revert resolved**：`ChangesView.Revert`（`[VCS]`）。Git 端用 `git restore --staged --worktree --source=…`（registry `git.can.use.restore.command=true`，`G/util/GitFileUtils.kt:223`）；另有 `Git.RevertResolved`。

### A7. 衝突解決

- **三方資料載入**：`GitMergeProvider.loadRevisions` → `GitMergeUtil.loadMergeData`（`G/merge/GitMergeProvider.java:109-112`）。`[Git]`
- **多檔批次對話框**：`VI/merge/MultipleFileMergeDialog.kt`。`[VCS]`
- **三欄 merge tool**：`DI/merge/*`（`TextMergeTool`、`MergeThreesideViewer`）。`[VCS]`
  - `ApplyNonConflictsAction`：套用所有非衝突變更。
  - `MagicResolvedConflictsAction`：自動解掉簡單衝突。
  - `RevertConflictResolutionAction`、`ShowDiffWithBaseAction`。
  - `LangSpecificMergeConflictResolver`：處理 imports 衝突。
  - `BinaryMergeTool`：二進位檔案。
- **整檔 Accept Yours／Theirs**：`Git.ChangesView.AcceptYours`／`AcceptTheirs`（在 `GX`）；`ConflictSide.OURS/THEIRS`（`GitMergeProvider.java:285-302`）。`[Git]`
- **rebase 中 yours／theirs 對調**：`GitMergeUtil.isReverseRoot`；registry `git.do.not.swap.merge.conflict.sides=false`（`GX:770`）。
- **解完後的處理**：執行 `git add` 或 `git rm`，必要時加 `--sparse`（`G/merge/GitMergeUtil.java:462-465`）。
- **入口**：Changes view 有「Merge Conflicts」節點；獨立的 conflicts tool window 預設關閉（registry `git.merge.conflicts.toolwindow=false`，`GX:737`）。
- **行尾衝突提示**：`G/crlf/*`。

### A8. Log

- **圖與排序**：`[VCS]`
  - 排序：Normal、Bek（「Standard」）、LinearBek、FirstParent（`VG`；定義在 `platform/vcs-log/graph-api/src/.../PermanentGraph.kt:51-73`）。
  - 折疊／展開 linear branches、顯示 long edges（`VL/ui/actions/Collapse*`、`ShowLongEdgesAction`）。
- **Filters**（`VL/ui/filter/*`、`VL/visible/filters/*`）：branch、user（含 regex）、date、structure（路徑）、text（Regex／Match Case）、hash、revision、range（`a..b`）、parent（排除 merges）、root。`[VCS]`
- **Highlighters**：My Commits、Merge Commits、Current Branch、Indexed Commits（`VL/ui/highlighters/*`）；Git 另加 Not Cherry-Picked（`GX:881`）。
- **欄位與面板**：
  - 欄位：Root names、Tag names、Compact references、Commit timestamp、Signature。
  - 面板：details、diff preview（放右側或下方）、「Show Changes to Parents」。
  - 其他：Go to Hash/Branch/Tag、開新 tab、log 內 Branches 面板、commit links 渲染、issue navigation。
- **多 repo**：`VcsLogMultiRepoJoiner`、`DV/*`；也可以開外部 repo 的 log（`Git.Log`，`G/log/GitShowExternalLogAction.kt`）。
- **右鍵選單**（`GX` 的 `Git.Log.ContextMenu` 群組）：checkout revision、從 commit 建 branch／tag、Push up to here、Reset、Revert、Cherry-pick、Interactive Rebase from here、Squash、Fixup、Reword、Drop、Undo、Create Patch、Compare Versions。
- **Show Repository at Revision**：`G/log/GitBrowseRepoAtRevisionAction.kt`＋`GitDirectoryVirtualFile.kt`。`[Git]`
- **log 內的 Branches 面板（dashboard）**：`G/ui/branch/dashboard/*`（registry `git.log.branches.use.async.tree.model=true`）。

### A9. File history、Blame、Compare、Patches

- **File history**：預設使用新版、以 log 為基礎的實作（registry `vcs.new.history=true`）。`[VCS]`
  - 相關 registry：`vcs.history.refine`、`vcs.history.remove.trivial.merges`。
  - 支援目錄歷史（`SUPPORTS_LOG_DIRECTORY_HISTORY`）。
  - 路徑：`VL/history/*`；Git 端 fallback：`G/history/GitFileHistory.kt`、`GitLogHistoryHandler.kt`
- **選取範圍的歷史**（`Vcs.ShowHistoryForBlock`）：`VI/history/impl/VcsSelectionHistoryDialog.java`＋`BlockLoader`。`[VCS]`
  - 在 `plugins/git4idea` 內 grep 不到 `-L`。推測是平台端逐個 revision 比對得出的（`[inference]`）。
- **Annotate／blame**：
  - 指令：`git blame --porcelain -l -t --encoding=UTF-8`（`G/annotate/GitAnnotationProvider.java:266`）。`[Git]`
  - 選項：忽略空白 `-w`（`ANNOTATE_IGNORE_SPACES=true`）；偵測移動 `-M`（INNER）或 `-C`（OUTER）（`GitVcsApplicationSettings.AnnotateDetectMovementsOption`）。
  - annotate 前一個或指定 revision：`VI/actions/AnnotatePreviousRevisionAction.java`、`AnnotateRevisionAction.java`。`[VCS]`
  - shallow repo 會顯示警告並提供 unshallow 動作（`GitAnnotationWarnings.kt`）。
  - code author inlay hints：`platform/vcs-impl/lang/src/.../VcsCodeVisionProvider.kt`。
- **Compare**：`Compare.SameVersion`、`LastVersion`、`Selected`、`Specified`（在 `VA`）。`[VCS]` 檔案與資料夾 diff：`DI/tools/dir/DirDiffTool.java`、`DI/tools/combined/*`。
- **Diff viewer**：`[VCS]`
  - 模式：side-by-side、unified（`DI/tools/fragmented/*`）、binary、三方。
  - 忽略空白策略：`ComparisonPolicy {DEFAULT, TRIM_WHITESPACES, IGNORE_WHITESPACES}`（`UD/diff/comparison/ComparisonPolicy.kt`）。
  - 可設定外部 diff tool（設定頁 `diff.external`）。
- **Patches**：`[VCS]`
  - 建立：`ChangesView.CreatePatch`、`CreatePatchFromChanges`、`CreatePatchToClipboard`；輸出 `diff --git` 格式（`VI/changes/patch/GitPatchWriter.java`）。
  - 套用：`ApplyPatchAction`、`ApplyPatchFromClipboardAction`，由 IDE 自己的 patch 引擎處理（`VI/changes/patch/*`）。
  - 在 `GitCommand` 清單與 grep 結果裡，都找不到 `git apply` 或 `git format-patch` 的使用。

### A10. Worktrees、Local History、搜尋、其他

- **Worktrees**：`[Git]`
  - 功能：列出、新增、開啟、刪除、prune、lock／unlock、開 review。新增時可依 `.worktreeinclude` 複製 config 類別並執行 setup script。
  - 路徑：`G/workingTrees/*`（38 個檔案）；action 為 `GX` 的 `Git.Show.WorkingTrees`、`Git.WorkingTrees.*`
  - registry：`git.enable.working.trees.feature=true`（`intellij.vcs.git.shared.xml:77`）
- **Local History**：IDE 自己的檔案層級歷史，不依賴 git，存在 `<system>/LocalHistory`（`platform/lvcs-impl/src/com/intellij/history/integration/LocalHistoryImpl.kt:108`）。`[VCS]`
  - 與 git 唯一的關聯：Git 在 push、merge、stage commit、apply-changes 前後放 system label（`G/push/GitPushOperation.java:181,200`、`G/actions/GitMergeAction.java:113,262`、`G/index/GitStageCommitter.kt:71`、`G/applyChanges/GitApplyChangesProcess.kt:147`）。
  - `plugins/git4idea/localHistory` 模組只放了 action group 的引用。
- **Search Everywhere**：可搜 commits、branches、tags（`G/search/*`）；有 Git 搜尋範圍（tracked、ignored）；依訊息搜 commit 由 registry `git.search.everywhere.commit.by.message` 控制。
- **其他**：
  - Git console 輸出：`G/commands/GitCommandOutputConsolePrinter.kt`
  - terminal 指令補全：`plugins/git4idea/terminal/*`
  - Git hosting 連結（Open in browser、Copy link）：`plugins/git4idea/hosting/*`、`GX` 的 `Git.Hosting.*`
- **遠端開發拆分架構**：`backend`／`frontend`／`shared`／`rpc.backend`。RPC 介面有 `GitWidgetApi`、`GitRepositoryApi`、`GitIncomingOutgoingStateApi`、`GitUiSettingsApi`、`GitOperationsApi`（`GX:1061-1069`）。
- **另案**（只列名稱）：
  - `plugins/github`（`github-core`、`github-git`、`github-json`、`github-tracker`）：PR 檢視、Gist、fork sync、share project。
  - `plugins/gitlab`（`gitlab-core`、`gitlab-git`、`gitlab-yaml`、`frontend`、`shared`）：MR 檢視。

### A11. 認證與 SSH

- **HTTP 認證**：`[Git]`
  - 把 `GIT_ASKPASS` 指向 IDE 的 helper（`plugins/git4idea/rt/src/git4idea/http/GitAskPassApp.java`；`G/commands/GitHandlerAuthenticationManager.java:85-170`）。
  - 預設會把 `credential.helper=` 清空，除非使用者勾選「Use credential helper」（`:93`）。
  - 認證失敗最多重試 2 次（`GitImplBase.java:123-138`）。
  - Windows 有 schannel 選項（advanced `git.use.schannel.on.windows`）。
- **SSH**：
  - 只用原生 OpenSSH。透過 `SSH_ASKPASS`＋`SSH_ASKPASS_REQUIRE=force`，並以 `DISPLAY` 作為 fallback（`:196-210`）。
  - 沒有 TTY 時用 `setsid` 執行（registry `git.use.setsid.for.native.ssh=true`）。
  - Git 設定頁（`G/config/GitVcsPanel.kt:233-351`）沒有 SSH executable 的選項。
- **憑證儲存**：`GitRememberedInputs` 加上 PasswordSafe（`GX:50` 依賴 `platform/credentialStore`）。
- `.gitignore` 與 credential helper 見上方；未找到全域 gitignore 的專屬支援。

### A12. Git 設定頁

設定頁位在 `Version Control | Git`，id 為 `vcs.Git`（`G/config/GitVcsPanel.kt:149-351`）。

- **一般**：Git executable 路徑（偵測、下載、WSL）；Mark ignored directories as excluded。
- **Commit 群組**：
  - Enable staging area
  - CRLF 警告
  - detached HEAD 或 rebase 中 commit 的警告
  - 大檔警告（大於 N MB；N 可設 1–100000，預設 50）
  - 問題檔名警告
  - cherry-pick 後綴
  - GPG 簽署（每個 root 各一把 key）
- **Push 群組**：
  - 被拒時自動 update
  - Commit and Push 時顯示 Push dialog，可限定只對 protected branches
  - Protected branches regex 清單，可由 GitHub 同步
- **Update 群組**：
  - Update method：merge／rebase／branch default
  - 清理工作樹的方式：Stash 或 Shelve
  - 過濾 Update Project 資訊的路徑
  - 自動 fetch（`FETCH`）、檢查遠端 incoming（`LS_REMOTE`），兩者受 advanced `git.update.incoming.outgoing.info` 控制（預設 true）
- **其他**：多 repo 同步分支操作；fetch tags 模式（Auto／Sync／Always／Never）；Use credential helper。
- **Stash 群組**：合併 Stashes 與 Shelves；Show Diff 時對 local 或對 parent。
- **Cherry-Pick 群組**：只在 registry `git.cherry.pick.use.git.sequencer` 開啟時顯示，可選空 commit 要跳過或建立。
- **沒在設定頁、但會持久化的選項**（`GSH/config/GitVcsOptions.kt`、`G/config/GitVcsApplicationSettings.java:24-37`）：
  - `SIGN_OFF_COMMIT`、`SET_USER_NAME_GLOBALLY`、`RESET_MODE`、`PUSH_TAGS`
  - `SHOW_RECENT_BRANCHES`、`SHOW_TAGS`、`FILTER_BY_ACTION`／`REPOSITORY_IN_POPUP`
  - `RECENT_PUSH_TARGETS`、`PREVIOUS_COMMIT_AUTHORS`（上限 16）
  - `ANNOTATE_IGNORE_SPACES`、`ANNOTATE_DETECT_INNER_MOVEMENTS`、`SHOW_DROP_COMMIT_DIALOG`
  - 另有 `gcAuto`、`coreLongpaths`、`coreUntrackedCache`、`coreFsMonitor`、`featureManyFiles`，預設都是 null（見 B4）。
- **Git 的 Advanced Settings**（`GX:666-676`）：

  | 設定 | 預設值 |
  | --- | --- |
  | clone 時 recurse submodules | true |
  | `git.read.content.with` | FILTERS |
  | push 用 force-with-lease | true |
  | update 時顯示 incoming／outgoing 資訊 | true |
  | update 資訊自動開啟 | true |
  | commit 時不執行 hooks | false |
  | Windows 使用 schannel | false |
  | branch cleanup 符號 | `"-"` |
  | 最近專案顯示分支 | ALWAYS |

- **VCS 通用的 advanced 設定**（`VcsExtensions.xml:351-357`）：`vcs.annotations.preload=false`、`vcs.process.ignored=true`、`vcs.commit.tool.window=true`、`vcs.non.modal.commit.toggle.ui=false`、`vcs.push.all.with.commits=false`、`vcs.allow.force.push.without.confirmation=false`。
- **平台通用設定頁**（`VI/configurable/*`）：Directory Mappings（`project.propVCSSupport.Mappings`）、General／Confirmation、Commit（`CommitDialogConfigurable`）、Shelf（`ShelfProjectConfigurable`）、Issue Navigation、Log（`vcs.log`）、Diff & Merge（`diff.base`、`diff.external`）。

---

## B. 效能架構

### B1. 讀取 git log

- **第一階段：first block**
  - `readFirstBlock` 執行 `git log HEAD --branches --remotes --max-count=<2×N>`，N 來自 registry `vcs.log.recent.commits.count=1000`（`REG:703`）。
  - 為什麼讀兩倍：程式註解說是為了在 IDE 內重新排序，而且「-1000 與 -2000 速度差不多」（`G/log/GitLogProvider.kt:113`）。
  - 刻意不加 `--tags`，因為 tag 多時會明顯變慢（`GitLogProvider.kt:116-117` 註解）。
  - 讀完後用 `VcsLogSorter.sortByDateTopoOrder` 在 IDE 內排序，再取前 N 筆。
  - 預設走 `GitLogExperimentalProvider`（registry `git.log.provider.experimental.refs.collection=true`，`GX:943`）。這個指令不請求 refs、不 decorate、不排序；refs 改從 `GitRepository` 與 tags holder 取得。
- **第二階段：全量 hash**
  - `readAllHashes` 執行 `HEAD --branches --remotes --tags --date-order`（`GitLogUtil.LOG_ALL`，`G/history/GitLogUtil.java:75`；`GitLogProvider.kt:214`）。
  - 每筆只取 `%H %P %ct`，另附 `%an %ae` 與 `%d`（`readTimedCommits`，`GitLogUtil.java:79-153`）。
  - 在 `plugins/git4idea` 內 grep `--topo-order`，只出現在 `G/actions/addCommit/GitAddCommitToRemoteBranchOperation.kt:227`，與讀 log 無關。也找不到 `commit-graph`、`core.commitGraph`、`multi-pack-index`。
- **Format 字串**：
  - 用 `--pretty=format:`，以 `%x01%x01` 當 RECORD_START、`%x02%x02` 當 ITEMS_SEPARATOR、`%x03%x03` 當 RECORD_END（`G/history/GitLogParser.java:56-59`）。
  - 解析出錯時，會在分隔符尾端追加隨機控制字元，避免和訊息內容撞到（`generateRandomSequence`，`:233`）。
  - 固定加 `--encoding=UTF-8`。所有指令預設帶 `-c core.quotepath=false -c log.showSignature=false`（`G/commands/GitHandler.java:166-167`）。
- **串流解析**：`GitLogOutputSplitter` 逐行把輸出交給 parser，不累積整份輸出（類別註解：「does not store output in order to save memory」）。
- **Details 延遲載入**：
  - 單一 commit 的完整 details（含變更檔案）用 `git log --no-walk=unsorted --stdin --name-status` 讀取，hash 從 stdin 餵入（`G/history/GitDetailsCollector.kt:35-44`）。
  - 會加 `-M`（`GitCommitRequirements.commandParameters`）。merge commit 預設是 DIFF_TO_PARENTS，每個 parent 各一筆 record。
  - 快取：
    - `CommitDetailsGetter` 用 Caffeine，上限 10,000 筆（`VL/data/CommitDetailsGetter.kt:25-27`）。
    - `MiniDetailsGetter`（列表用的 metadata）上限 10,000 筆；載入執行緒是 `SequentialLimitedLifoExecutor`，最多 10 個並行任務（`MAX_LOADING_TASKS=10`，`MiniDetailsGetter.kt:40-41,188`）。
  - `MiniDetailsGetter` 先查 index，沒被索引的才交給 provider 讀（`:100-140`）。
- **刷新**：
  - `VcsLogRefresherImpl` 先 `loadFirstBlock`（`:236`）讓畫面有東西，再 `readFullLogFromVcs`（`:390`）。
  - 之後的刷新只讀「最近 N 筆」，用 `VcsLogJoiner` 與舊的完整 log 合併；合不起來（`VcsLogRefreshNotEnoughDataException`）才全量重讀（`:426-444`）。
  - 可選的 overlay：`vcs.log.small.data.pack.commits.count=100`（`REG:706`）。
  - 觸發來源是 repo 變化事件（`GitLogProvider.subscribeToRootRefreshEvents`）。log 不可見時刷新會延後（`PostponableLogRefresher`；registry `vcs.log.keep.up.to.date=true`）。

### B2. VcsLog persistent index

- **預設狀態**：
  - `vcs.log.index.enable=false`（`REG:714`）。commit `e55bc60dfa` 的訊息是「[vcs-log] disable indexing by default」，2023-09-05 由 true 改成 false（以 GitHub API 查證）。
  - 專案層另有 `VcsLogSharedSettings.IS_INDEX_ON=true`（`VL/impl/VcsLogSharedSettings.java:26`）；兩者都為真才會索引（`Project.isIndexingEnabled`，在 `VL/data/index/VcsLogIndexUtils.kt` 結尾）。
  - 使用者可在 log 工具列按「Enable … Log Indexing」（`ResumeIndexingAction`），同時打開 registry 與專案設定（`enableIndexing`）。
  - 關閉時 `VcsLogData.createStorageAndIndex` 回傳 `storage to EmptyIndex()`（`VL/data/VcsLogData.kt:380`）。
  - 即使關閉，仍會保留兩份 persistent 資料：hash↔int 的 `PersistentBTreeEnumerator`（`VcsLogStorageImpl`，VERSION=8）與 refs enumerator。
- **存什麼**：
  - PHM 後端（`VL/data/index/PhmVcsLogStorageBackend.kt:49-59`）：messages、parents、committers、timestamps（author time 與 commit time）、renames、訊息的 trigrams、paths（路徑↔commit，含 rename map）、users。
  - SQLite 後端 schema（`SqliteVcsLogStorageBackend.kt:54-97`）：`log(commitId, message, authorTime, commitTime, isCommitter)`、FTS5 trigram 虛擬表 `fts_message_index`、`parent`、`rename`、`user`、`path`、`path_change`、`commit_hashes`。
- **後端選擇**：`vcs.log.index.sqlite.storage=false`（`REG:730`；`VcsLogData.kt:353`），所以預設是 PHM，SQLite 要另外開。
- **位置與版本**：
  - 位置：`<PathManager.getSystemPath()>/vcs-log/<專案名前7碼>.<logId>/`（`VL/util/PersistentUtil.kt:21,32-34`）。
  - 版本：PHM 為 `VcsLogPersistentIndex.VERSION=18`；SQLite 檔名 `vcs-log-v2-18.db`。
- **建立與增量更新**：
  - 所有已載入的 commit 由 `VcsLogCommitDataConsumerImpl.storeData` 標記待索引；全量載入完成後呼叫 `index.scheduleIndex(onFullReload)`（`VL/data/VcsLogCommitDataConsumer.kt`）。
  - 新建儲存時，用一條 `git log`（`LOG_ALL`＋`--name-status`，`G/log/GitLogIndexer.readAllFullDetails`）做全量索引。
  - 增量時，每批 20,000 個 hash（`BATCH_SIZE=20_000`，`VcsLogPersistentIndex.java:401`）透過 `--no-walk --stdin` 送出；每 15,000 筆 flush 一次，SQLite 是 5,000 筆（`:410`）。
- **資源控制**：
  - 索引用的 `git log` 以低優先權執行：`GitLogUtil.createGitHandler(..., lowPriorityProcess)`，Unix 用 `nice -n 10`，Windows 用 `start /b /low`（`G/config/GitExecutable.kt:57-67`）。
  - 執行緒設為 `Thread.MIN_PRIORITY`（`VcsLogPersistentIndex.java:343`）。
  - 偵測到系統忙碌或省電模式時暫停並延後（registry `vcs.log.index.postpone.on.heavy.activity.or.power.save=true`）。
  - 超時上限 `vcs.log.index.limit.minutes=20`（`REG:724`）。超時就把該 root 加進 `VcsLogBigRepositoriesList`（持久化在 cache）並取消索引。
  - 閒置時才啟動：`vcs.log.index.start.on.idle.delay.minutes=-1`（預設關閉）。
  - 連續錯誤達 30 次，也會加進 big repo 清單。
- **哪些 filter 會用 index**（`IndexDataGetter.canFilter`，`VL/data/index/IndexDataGetter.java:162-176`）：
  - 會用：text、user、structure（路徑；但若路徑是目錄而且就是 root，則不能用）。
  - 不用：date、branch、parent 等。
  - 訊息搜尋：先取 trigram 交集，再用 `filter.matches(message)` 過濾。少於 3 個字元、取不出 trigram 的字串改用線性掃描（`noTrigramSources`，`PhmVcsLogStorageBackend.kt:290-311`；SQLite 端的註解在 `SqliteVcsLogStorageBackend.kt:346-373`）。
  - 流程：`VcsLogFiltererImpl.filterByDetails` 讓已索引的 root 走 index、其他 root 走 `git log`，兩邊結果取聯集（`:190-226`）。
- **沒有 index 時走 git**：
  - `CommitCountStage.INITIAL = (5, 100, 2000, MAX)`（`VL/visible/CommitCountStage.kt:19`），逐階段放大 `--max-count`。第一階段先在記憶體裡已載入的 metadata 上過濾（`VcsLogFiltererImpl.kt:253-269`）。
  - git 參數組合在 `G/log/GitLogProvider.getGitLogParameters`：`--after`／`--before`、`--grep`＋`--extended-regexp`／`--fixed-strings`、`--regexp-ignore-case`、`--author=`、`--first-parent`、`--min-parents`／`--max-parents`、`--full-history --simplify-merges -- <paths>`。
- **其他用途**：file history（見 B5）、blame 訊息補全、Search Everywhere（`git.search.everywhere.commit.by.message`）、cherry-pick 偵測（`git.log.use.index.for.find.picked.commits`）。

### B3. Graph layout（`platform/vcs-log/graph`）

- **Permanent graph**：由 `PermanentGraphImpl.newInstance`（`VG/impl/facade/PermanentGraphImpl.kt`）用 `PermanentLinearGraphBuilder` 建出。
  - 節點只有一個 parent，而且 parent 正好是下一列時，標記在 `simpleNodes`（BitSet），不存邊（`PermanentLinearGraphBuilder.java:55-67`）。
  - 其餘的邊存進 `int[] longEdges`，再轉成 `IntImmutableList`。
  - commit 以 int（storage index）表示；節點與 commit id 的對應存在 `PermanentCommitsInfoImpl`。
- **Layout index**：`GraphLayoutBuilder.build`（`GraphLayoutBuilder.kt:58`）先排序 head，再對每個 head 做 DFS，為節點指派 `layoutIndex`（`IntArray`）。
  - head 的排序依 `headCommitsComparator`（`VcsLogRefManager.getBranchLayoutComparator`）：重要分支靠左，並沿用顏色。
- **排序（IntelliSort／Bek）**：`bekIntMap` 是 lazy（`PermanentGraphImpl.kt:44`），只有選用 Bek 或 LinearBek 時才計算（`BekSorter` → `BekBranchCreator`／`BekBranchMerger`）。
- **Visible graph**：`createVisibleGraph` 依選項組出 controller 鏈（`PermanentGraphImpl.kt:51-83`）。

  | 選項 | controller |
  | --- | --- |
  | 預設 | `BaseController` |
  | Bek | `SortedBaseController` |
  | 折疊 | `CollapsedController` |
  | 篩選 | `FilteredController` |
  | 只看 first parent | `FirstParentController` |
  | LinearBek | `LinearBekController` |

  折疊後的圖用 `CollapsedGraph`，可見性用 `UnsignedBitSet`，邊存在 `EdgeStorage`（`VG/collapsing/*`）。
- **繪圖元素逐 row 產生並快取**：
  - `PrintElementGeneratorImpl` 用 `SLRUMap(100, 200)` 快取（`:39,282`）。
  - 長邊門檻 `LONG_EDGE_SIZE=30`；開啟「Long Edges」時改為 1000／250（`:277-281`）。
  - 推薦寬度只取樣前 N 個節點計算。
- **小資料包**：
  - 第一階段的圖只含前 N 筆；`PermanentGraphImpl` 用小於 -2 的 `NOT_LOAD_COMMIT` 邊 id 表示「parent 還沒載入」。
  - 完整資料載入後，再建完整的圖（`VcsLogGraphDataFactory.buildData(..., isFull)`）。

### B4. Status 與 changes 刷新

- **觸發來源**：
  1. VFS 事件 → `VcsDirtyScopeVfsListener`（`VI/changes/VcsDirtyScopeVfsListener.kt:36-90`）。move 與 rename 會同時標記新舊路徑。IDE 的 VFS 本身靠 IDE 原生的 file watcher。
  2. `.git` 內的檔案變化：`GitRepositoryUpdater` 用 `WatchRoots.watch(path, true)` 監看 `.git`（`G/repo/GitRepositoryUpdater.kt:35`）。index、HEAD、ORIG_HEAD 或 branch 檔變化時，呼叫 `VcsDirtyScopeManager.rootDirty(root)` 把整個 root 標髒，並 `untrackedFilesHolder.invalidate()`（`:155-163`）。
- **去抖**：`UpdateRequestsQueue.schedule` 延遲 300ms（`VI/changes/UpdateRequestsQueue.java:92`）；untracked holder 用 `SingleTaskRunner.delayedTaskRunner(…, 500.milliseconds)`（`G/repo/GitUntrackedFilesHolder.kt:79`）。
- **dirty scope 結構**：
  - `VcsDirtyScopeImpl` 存兩組資料：單檔 set 與遞迴目錄 set。
  - `RootDirtySet`：同一目錄下超過 30 個子項時，折疊成父目錄；總量超過 50,000 時直接視為全部都髒（`VI/util/paths/RootDirtySet.java:32-33`）。
- **status 指令**：
  - 追蹤中檔案的變更用 `git status --porcelain -z --untracked-files=no [--ignored=no] [-- <paths>]`（`G/index/GitIndexStatusUtil.kt:117-137`、`G/status/GitStagingAreaHolder.java:refresh`）。
  - pathspec 超過 7600 字元（`VcsFileUtil.FILE_PATH_LIMIT`）時，`GitUtil.createHandlerWithPaths` 會丟掉路徑清單、重掃整個 root（`GitUtil.java:1143-1155`）。
  - 為了偵測 rename，已知為 A、D、R 的路徑一律加進 pathspec（`GitStagingAreaHolder.collectDirtyPathsPerRoot`）。
  - 狀態為 `MM` 時，額外跑 `git diff --name-status` 確認是否真的有差異（`GitChangesCollector.collectStagedUnstagedModifications`）。
  - 這時不載入檔案內容：`GitContentRevision` 是延遲載入，`reportModified` 等只建立 revision 物件。
- **untracked 與 ignored 另走一條路**：
  - `GitUntrackedFilesHolder` 執行 `git status --porcelain -z --untracked-files=all [--ignored=matching]`。是否加 `--ignored` 由 advanced `vcs.process.ignored`（預設 true）決定（`:335,342`）。
  - 整個 root 都髒時掃全部；只有部分檔案髒時，只帶 dirtyFiles 的路徑。
  - 結果存在 `CopyOnWriteFilePathSet`／`RecursiveFilePathSet`。IDE 重啟後 holder 會重新初始化（`isEverythingDirty = true`）。
- **fsmonitor 與 untracked cache**：
  - 未找到預設啟用它們的程式碼。
  - `GitSetupProjectConfig`（`G/config/GitSetupProjectConfig.kt:50-66`）只在 `GitVcsOptions` 的 `coreFsMonitor`、`coreUntrackedCache`、`featureManyFiles`、`gcAuto`、`coreLongpaths` 有值時，才對還沒設定該 key 的 repo 執行 `git config --local`。這些欄位預設都是 null，而且在 git4idea、vcs-impl、vcs-log 內找不到寫入它們的程式碼。
  - 唯一跟 fsmonitor 有關的邏輯，是專案關閉時停止 daemon：`git fsmonitor--daemon stop`（registry `git.stop.fsmonitor.on.project.close=true`，`G/repo/GitFsMonitorProjectCloseListener.kt:103`）。
  - `GitIndexStatusUtil.kt:158` 會略過 git-for-windows 2.33 實驗性 fsmonitor-daemon 的除錯輸出。
- **讀取 repo 狀態**：
  - `GitRepositoryReader` 直接讀檔取得 HEAD、merge／rebase 狀態檔、`shallow`、`.gitmodules`。
  - 分支清單預設用 `git for-each-ref refs/heads/** refs/remotes/** --format=%(refname)\t%(objectname)\t%(HEAD)`（registry `git.read.branches.from.disk=false`，`GitRepositoryReader.kt:45,228-231`）。
  - config 解析 `git config` 的輸出；tags 另用 `for-each-ref refs/tags/**`（`GitRepositoryTagsHolderImpl.kt:72-75`）。
  - 另有一個最小的 reftable 解析器，只讀 HEAD，給 welcome screen 用（`GitReftableReader.kt`）。
- **未存檔的文件**：`NonChangedHolder.feedBuilder` 把 IDE 裡已修改但未存檔的文件視為 MODIFIED（`GitChangeProvider.java:177-205`；`isModifiedDocumentTrackingRequired()=true`）。

### B5. Blame 與 file history 加速

- **Blame**：
  - 指令：`git blame --porcelain -l -t --encoding=UTF-8 [-w] [-M|-C] <rev> -- <file>`（`GitAnnotationProvider.java:266-292`），用 `GitBinaryHandler` 執行以保留 CR。
  - 快取：`VcsHistoryCache` 用 Caffeine，key 是（path, vcsKey, revision）（`platform/vcs-api/src/com/intellij/openapi/vcs/history/VcsHistoryCache.kt:17-24`）。

    | 項目 | 上限 | 開啟 preload 時 |
    | --- | --- | --- |
    | annotation | 10 筆 | 50 筆 |
    | history | 10 筆 | 50 筆 |
    | lastRevision | 50 筆 | 50 筆 |

  - 沒有指定 revision 時，先算出 annotation，再在背景計算 last revision 並補進快取（`GitAnnotationProvider.java:160-182`）。
  - commit 訊息優先從 log index 補（`loadCommitMessagesFromLog`，約 `:330`）；沒有 index 才在背景載入 file history（`loadFileHistoryInBackground`）。
  - 預載由 advanced `vcs.annotations.preload=false` 控制，實作是 `AnnotationsPreloader`。
  - 沒有用 `--incremental`，也未找到 blame 專用的磁碟快取。
- **File history**：
  - registry：`vcs.new.history=true`、`vcs.history.use.index=true`。
  - root 已被索引、而且 data pack 是完整的，才走 index（`FileHistoryFilterer.kt:382-384`）。
  - 否則走 VCS handler：`GitFileHistory` 先跑 `git log [--full-history --simplify-merges] -- <file>`，再用 `git show -M --follow --name-status <first_commit>` 追 rename，一路追到 ADD 為止（類別註解在 `G/history/GitFileHistory.kt:30-58`；`git.file.history.full=true`）。
  - 走 index 時，用 `IndexDataGetter.createFileHistoryData` 加 rename map 查路徑；index 是空的就退回 VCS（`FileHistoryFilterer.MyWorker.filter`，`:148-196`）。

### B6. 指令執行

- **類別階層**：`GitLineHandler`／`GitBinaryHandler` → `GitTextHandler` → `GeneralCommandLine`＋`KillableProcessHandler`（`G/commands/GitHandler.java`、`GitTextHandler.java`）。輸出預設用 blocking reader 讀取（registry `git.blocking.read=true`，`GitTextHandler.java:172-175`）。
- **啟動前**：先 `ManagingFS.flushPendingUpdates()`（`GitHandler.java:495`）；專案未受信任就拒絕執行（`:479-488`）。
- **環境變數**（`GitHandler.prepareEnvironment`，`:538-555`；`GitImplBase.java:204-270`）：
  - 繼承 IDE 的 `EnvironmentUtil` 環境。
  - `LC_ALL=en_US.UTF-8`、`LANGUAGE=""`（`platform/vcs-impl/src/com/intellij/vcs/VcsLocaleHelper.kt:42-45`），可用 registry `git.executable.locale` 覆寫。
  - `INTELLIJ_GIT_EXECUTABLE=true`，讓 hooks 偵測得到。
  - `GIT_TRACE*` 預設清成 0 或空；registry `git.execution.trace` 的位元遮罩可把 trace 導到 `gittrace.log`。
  - `GIT_OPTIONAL_LOCKS=0`：git ≥ 2.15 且 registry `git.use.no.optional.locks=true` 時才設（`GitImplBase.java:206-210`；`GitVersionSpecialty.kt:93`），避免 `status` 跟終端機裡的 rebase 搶 `index.lock`。
  - 專案 context 的環境（例如 python venv）：registry `git.use.env.from.project.context=true`，並移除 `PS1`。
  - 認證 helper、`GIT_EDITOR`、`SSH_ASKPASS` 等見 A6、A11。
- **鎖**：
  - `GitCommand` 靜態登記每個指令的鎖策略：`READ`、`READ_OPTIONAL_LOCKING`、`WRITE`（`GitCommand.java`，約 `:25-100`）。
  - 只有 WRITE 指令會取專案層的 `ReadWriteLock.writeLock()`，等待時用 `ProgressIndicatorUtils.awaitWithCheckCanceled`（`GitImplBase.java:422-435,455-461`）。
  - 舉例：`fetch`、`push`、`log`、`status` 是 READ；`ls-files` 是 READ_OPTIONAL_LOCKING，只有無法抑制 optional locks 時才當成 WRITE；`commit`、`checkout`、`reset`、`rebase`、`merge`、`stash` 是 WRITE。
  - 讀取指令之間沒有全域併行上限，併行程度由呼叫端的 coroutine 或背景任務決定。
- **取消與長時間指令**：
  - `waitForProcess` 每 50ms 執行一次 `myHandler.waitFor(50)` 和 `ProgressManager.checkCanceled()`。
  - 取消時呼叫 `destroyProcess`，進入 `executeNonCancelableSection`：先 soft kill，最多等 60 秒，再硬殺（`GitTextHandler.java:30-31,110-148`）。
  - 呼叫端持有 read lock 時，等待 kill 的工作會移到 pooled thread，避免卡住取消。
  - `logTime`：執行時間超過 `LONG_TIME` 的指令記成 INFO（`GitHandler.java:459-472`）。
- **路徑太多**：pathspec 超過 7600 字元（`VcsFileUtil.FILE_PATH_LIMIT`）時分批執行，或改用 `--pathspec-from-file`（registry `git.use.pathspec.from.file=true`，`GitFileUtils.kt:261-285`）。
- **低優先權與無 TTY**：`withLowPriority`（`nice` 或 `start /low`）、`withNoTty`（`setsid`）。
- **並行 fetch**：
  - `git.parallel.fetch.threads=-3`，代表「repo 數量，但不超過 2×CPU 核心」；SSH multiplexing 時上限 10（`GX:708-714`、`GitFetchSupportImpl.kt:159-190`）。
  - 更新其他 worktree 的分支，並行上限 10（`git.branches.update.other.worktree.max.concurrent`）。
- **定期檢查 incoming**：預設策略是 `LS_REMOTE`，每 20 分鐘一次（`git.update.incoming.info.time=20`；`GX:919`；`G/branch/GitBranchIncomingOutgoingManager.java:252-255,497`）；數量用 `git rev-list --count` 計算（`:526-531`）。自動 fetch 的策略是 `FETCH`。
- **簽章載入**：需要時才執行 `git log --format=%H %G? %GS %GF`，結果放在共享快取（`G/log/GitCommitSignatureLoader.kt:22-40`）。

### B7. Diff 計算

- **文字 diff 由 IDE 自己算**：
  - 呼叫路徑：`ComparisonManagerImpl`（`DI/comparison/ComparisonManagerImpl.java`）→ `ByLineRt`／`ByWordRt`／`ByCharRt`（`UD/diff/comparison/*`）→ `DiffIterableUtil.diff` → `Diff.buildChanges`（`UD/util/diff/Diff.kt`）。
  - 預設用 `MyersLCS.executeWithThreshold`：時間 O(ND)、記憶體 O(N)。門檻是 `max(20000 + 10·√(n1+n2), DELTA_THRESHOLD_SIZE=20000)`（`MyersLCS.kt`，約 `:69-73`；`DiffConfig.kt:10`）。
  - 超過門檻時丟 `FilesTooBigForDiffException`，改用 `PatienceIntLCS(…).execute(true)`（`Diff.kt:82-98`）；還是太大就丟 `DiffTooBigException`，在 UI 上提示。`DiffConfig.USE_PATIENCE_ALG=false`。
  - 計算前先剪掉共同的前綴與後綴，並用 `Enumerator` 把每一行轉成 int 再比較（`Diff.buildChanges`）。
- **git diff 的角色**：只用來取得檔案層級的清單與偵測 rename。
  - 指令：`git diff --name-status [-M] [--diff-filter=ADCMRUXT] <range>`（`G/changes/GitChangeUtils.java:415-425,495-511`）。
  - merge commit 會對每個 parent 各跑一次 `--name-status -M`（`:300-304`）。
  - 檔案內容用 `git cat-file [--filters|--textconv|-p] <rev>:<path>` 取得，方式由 advanced `git.read.content.with` 決定，預設 FILTERS（`G/util/GitFileUtils.kt:242-273`）。
- **三方 merge**：載入 index 各 stage 的內容後，由 IDE 的 `ComparisonMergeUtil`／`MergeThreesideViewer` 計算；`MergeResolveUtil` 提供簡單衝突的自動解法。
- **Gutter 與 partial changelist**：`LineStatusTracker` 用同一套 IDE 比較邏輯對照 base 內容（`VI/impl/LineStatusTrackerManager.kt`）。

### B8. 其他效能相關設計

- **Partial commit 不動工作樹**：
  - 先用 `git update-index --index-info` 把其他已 staged 的項目存起來，commit 後再還原（registry `git.commit.staged.saver.use.index.info=true`，`G/checkin/GitStagingAreaStateManager.kt:32`、`GitIndexInfoStagingAreaStateManager.kt`）。舊的做法是 reset 再 add。
  - 部分內容經 `hash-object`＋`update-index` 寫進 index（`G/index/GitIndexUtil.java:241,265`）。
- **In-memory 歷史編輯**：`GitObjectRepository` 用 plumbing 指令（`cat-file`、`commit-tree` 等）加上物件快取完成操作，不碰工作樹與 index（`G/inMemory/GitObjectRepository.kt:11-30`；`GitInMemoryInteractiveRebaseProcess.kt:34`）。
- **拆分架構**：`backend`／`frontend`／`shared`／`rpc.backend`，讓 UI 與執行分開（見 A10）。

---

## 證據缺口

- **index 的預設值**：registry 預設 false（`REG:714`，commit `e55bc60dfa`）。官方文件頁（investigate-changes）只寫有「Enable Git Log Indexing」按鈕，沒寫預設狀態。我只查了 repo 內搜尋與 GitHub code search，無法完整驗證各發行版在產品層的覆寫。
- **fsmonitor 與 untracked cache**：在 git4idea、vcs-impl、vcs-log 範圍內，找不到寫入 `GitVcsOptions` 這些欄位的程式碼。
- **未找到的項目**：
  - commit-graph、multi-pack-index、讀 log 時用 `--topo-order`、`git log -L`
  - sparse-checkout UI：`GitCommand.SPARSE_CHECKOUT` 有定義（`GitCommand.java:31`），但 grep 不到任何使用處；`--sparse` 只出現在 `GitMergeUtil.java:462-465`
  - partial clone（`--filter=`）、`git apply`、`git format-patch`
  - hooks 編輯 UI、全域 gitignore 的專屬設定
  - blame 的磁碟快取
  - 獨立的「set upstream」action
- **Block history**：Git 端不用 `-L` 是根據 grep 結果；實作位置的判斷屬於推論（`[inference]`）。
- **文件來源**：JetBrains 文件頁是用 WebFetch 讀的（由小模型摘要），只當成使用者可見功能的次要佐證，以原始碼為準。部分頁面本身沒涵蓋 interactive rebase、worktrees、SSH，這幾項只有原始碼證據。
- **沒有實際執行**：沒有跑過 IDE，所有行為都來自靜態閱讀。如果預設值被 `registry.properties` 以外的機制覆寫，我觀察不到。

## 假設

- 「IntelliJ IDEA Community」以 master 的 `intellij-community` 為準，不涵蓋 Ultimate 專屬功能。
- `platform/vcs-*`、`dvcs-*`、`diff-*` 視為「VCS 平台通用」，`plugins/git4idea` 視為「git 專屬」。
- A 節的「功能」以 action id、設定項、registry key 與 UI 類別為單位，不保證每一項在目前版本都看得到（例如 stash 和 working trees 都有 registry 開關）。
- B 節的行號，以研究用 clone 裡讀到的內容為準。

## 未涵蓋

- `plugins/hg4idea`、`plugins/svn4idea`、`plugins/space`：與 Git 無關。
- `plugins/github`、`plugins/gitlab`：依指示只列名稱與範圍。
- `platform/vcs-impl` 的 `lang`／`frontend`／`backend`／`shared` 內的 UI 細節、`IgnoredToExcludedSynchronizer`、issue navigation、task 整合：與核心 Git 子系統關聯低，沒有深入。
- 單元測試與效能測試（`performanceTesting`、`tests`）：沒有讀，所以沒有實測的延遲數字。
- `vcs-log/impl/bookmarks`、`PostponableLogRefresher` 的內部細節、`IdleVcsLogIndexer`：只確認存在與主要行為。
- JetBrains 文件只讀了 Git 章節的 9 個主要頁面，沒有逐頁驗證所有子頁。

## 研究資料位置

- `/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/beta2-research/intellij`（約 111MB）：用 `git clone --filter=blob:none --sparse --depth 1 --no-tags` 加 sparse-checkout 建立。checkout 的目錄有 `plugins/git4idea`、`plugins/git-modal-commit`、`plugins/git-features-trainer`、`platform/{vcs-log,vcs-impl,vcs-api,diff-impl,diff-api,dvcs-api,dvcs-impl,lvcs-impl,lvcs-api}`、`platform/util/diff`。
- `/Users/yuuzu/HanaokaYuuzu/App/Tauri/.yuzora-lanes/beta2-research/raw/registry.properties`：IntelliJ registry 預設值副本。
