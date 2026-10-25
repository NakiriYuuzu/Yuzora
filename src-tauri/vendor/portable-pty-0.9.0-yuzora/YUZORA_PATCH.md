# Yuzora macOS PTY open lifecycle patch

Base: the exact portable-pty 0.9.0 crate already locked by Yuzora, including its MIT license. Registry file hashes and the candidate diff are recorded in `.yuuzu/eval/perf-bug-loop-2026-10-20/experiments/076-atomic-pty-open/`.

On macOS, `openpty` returns master and slave descriptors before the crate sets `FD_CLOEXEC`. A process spawned concurrently can inherit the master and retain it after the native HERDR client closes, preventing the client from exiting and being reaped within the existing deadline.

The macOS branch follows Apple's `openpty(NULL, winsize)` setup, but opens both descriptors with `O_CLOEXEC`. It keeps the system terminal defaults, four window-size fields, controlling-terminal setup, spawn behavior and existing close/termination deadlines. Acquired descriptors have RAII ownership on every error path. `TIOCPTYGNAME` avoids requiring the newer `ptsname_r` symbol. Other Unix platforms and Windows keep their existing implementations; both macOS App and Intel macOS remote Host builds use the macOS branch.

A deterministic diagnostic in the experiment archive pauses before the original post-open CLOEXEC calls, execs a native FD holder, then calls the actual manager release. Before the fix, retaining master caused a timeout; closing only that descriptor while the holder stayed alive allowed the client to exit. After the fix, both original descriptors are absent in the holder, and every case is reaped before holder cooperation. Diagnostic hooks are not included in this vendored source.

The isolated native-client lifecycle test also checks that no PTY was inherited at startup, validates terminal defaults, geometry and bidirectional I/O, exercises allocation-failure cleanup, and retains its 20 warmups and 100 measured close cycles. This patch makes no performance claim and does not prevent a fork that has not executed a new program from temporarily retaining descriptors.
