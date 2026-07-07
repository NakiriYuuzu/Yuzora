# pi-acp issue 草稿:unhandled EPIPE crash when inner `pi` process dies

> 狀態:草稿,尚未送出。送出前請確認 pi-acp 的 issue tracker 位置與最新版本是否已修。
> 佐證資料:`~/.yuzora/logs-backup-20260707/yuzora-2026-07-06.jsonl`(agent-1,19:16)

## Title

Unhandled `EPIPE` in `_PiRpcProcess.writeLine` crashes the whole adapter when the inner `pi` process exits

## Body

**Environment**

- pi-acp: fetched via `bunx pi-acp@latest` on 2026-07-06(version at the time of crash unknown; 0.0.31 reproduces the code path)
- Node.js v25.8.1, macOS (darwin 25.5.0)
- Client: yuzora (Tauri app) speaking ACP over stdio

**What happened**

The ACP handshake completed (initialize responded on stdout). ~18s after spawn, the
adapter crashed with an unhandled `'error'` event when writing a line to the inner
`pi` RPC process whose stdin had already closed (the inner process had died):

```
node:events:486
      throw er; // Unhandled 'error' event
      ^
Error: write EPIPE
    at afterWriteDispatched (node:internal/stream_base_commons:159:15)
    ...
    at file:///…/bunx-501-pi-acp@latest/node_modules/pi-acp/dist/index.js:302:26
    at new Promise (<anonymous>)
    at _PiRpcProcess.writeLine (file:///…/bunx-501-pi-acp@latest/node_modules/pi-acp/dist/index.js:300:12)
Emitted 'error' event on Socket instance at:
    ...
  errno: -32, code: 'EPIPE', syscall: 'write'
```

Exit code 1;the ACP client only sees the process die with no protocol-level error.

**Expected**

- `_PiRpcProcess` should attach an `'error'` handler to the child stdin socket (or
  guard `writeLine` with a writability check), and
- when the inner `pi` process dies, surface a JSON-RPC error / session update to the
  ACP client instead of crashing the adapter process.

**Notes**

- The inner `pi` had earlier logged provider warnings (`YUUZU_API_KEY is missing;
  provider not registered`), so a plausible trigger is the inner process exiting
  early on configuration problems.
