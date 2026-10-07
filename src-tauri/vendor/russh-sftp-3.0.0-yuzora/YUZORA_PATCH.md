# Yuzora SFTP handle ownership patch

Base: the exact russh-sftp 3.0.0 crate locked by Yuzora, including its Apache-2.0 license. Original package hashes and checksum are recorded in `.yuuzu/eval/performance-loop-2026-10-02/experiments/051-sftp-directory-lifetime/registry-provenance.json`.

`SftpSession::read_dir` now owns an acquired directory through `DirectoryHandle`. A READDIR error or a dropped listing schedules one CLOSE. The existing native/wasm runtime abstraction keeps acquisition alive through caller cancellation, so a handle received later is also closed. Explicit normal close waits for acknowledgement; dropping that close future leaves its task responsible for finishing local handle accounting.

Directory iteration order, EOF handling, returned errors, and public API remain unchanged. A close error on the normal path remains an error. Cleanup after an earlier READDIR error is best effort and preserves the original error. Raw HANDLE reply ownership also covers request timeout and cancellation, as described below.

## File close accounting

Dropped files still enqueue CLOSE synchronously, before later caller requests. The response loop releases local handle capacity only for a successful acknowledgement, including when the waiter has timed out or been cancelled. If a file shutdown is already waiting for its CLOSE, dropping the file transfers that existing future to the runtime instead of sending a second CLOSE. A freshly queued Drop close no longer needs a detached acknowledgement waiter.

## Unclaimed HANDLE replies

Incoming HANDLE packets retain cleanup ownership until a request actually polls and consumes them. A timed-out or cancelled request, a failed response delivery, or a delivered response discarded before polling enqueues one CLOSE. Ordinary delivered handles pass ownership to their caller. Other packet types keep their existing behavior.

Cleanup uses the same request ID counter and outgoing packet limit, and holds only a weak transport sender. It creates no detached waiter or persistent abandoned-request entry. Unclaimed handles were never added to the local handle count, so the cleanup CLOSE acknowledgement is ignored instead of decrementing another live handle's capacity.

Cleanup remains best effort if the transport is closed, the server refuses CLOSE, or its supplied handle cannot fit its advertised packet limit. Counted CLOSE acknowledgements use the separate ownership described below.

## Directory packet accumulation

Directory entries accumulate directly into the existing ReadDir VecDeque. Each decoded packet reserves space and prepends its entries in reverse order, preserving the established reverse packet order and each packet's own entry order. This avoids rebuilding every earlier entry, intermediate batch arrays, and a final merge. EOF handling, metadata, dot filtering, paths, and directory cleanup remain unchanged.

Deque capacity grows amortized and may exceed the entry count. The current SshManager listing consumer immediately collects the iterator. CPU, RSS, footprint, peak memory, small-directory controls and repeated listing measurements are recorded in `.yuuzu/eval/performance-loop-2026-10-02/experiments/054-sftp-directory-accumulation/`; this patch does not claim every memory metric or a retained iterator buffer becomes smaller.

## Counted CLOSE acknowledgements

Pending requests distinguish CLOSE from other replies. A dropped or timed-out CLOSE releases its response sender and keeps only acknowledgement metadata in the existing request map. The response loop removes that entry and returns one unit of capacity only for a validated Status Ok. Rejected or unexpected replies do not free capacity; unsolicited replies and unclaimed-HANDLE cleanup cannot decrement another live handle. Transport shutdown clears the map. Other request cancellation still removes its entry immediately.

An unacknowledged CLOSE must retain its small metadata entry until a reply or transport shutdown; timeout alone cannot prove the remote handle was closed. This is intentional pending-operation state, with no detached waiter or timer retained. It does not establish a universal memory reduction. The close deadline and returned errors remain unchanged. Owned protocol, normal read/open/close, cancellation, timeout and disconnected-transport evidence is recorded in `.yuuzu/eval/performance-loop-2026-10-02/experiments/057-sftp-close-ack-accounting/`.
