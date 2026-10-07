# Yuzora channel lifecycle patches

This directory contains the crates.io russh 0.62.7 package (Apache-2.0), from upstream commit a3766cca2223f851df786e88f823ea08dabfbdea. The registry package SHA-256 is 9decb68e4e44e1079700e54f17c8f23806ec53d7e0db73ab1c71d9dabc666812. Upstream: https://github.com/Eugeny/russh (the package's warp-tech/russh URL redirects there). Original source copyright headers and package metadata are preserved. LICENSE-APACHE contains the Apache License 2.0 text.

Yuzora modifications (2026-10-03): src/client/mod.rs and src/client/encrypted.rs, with pure ownership regression tests in src/client/pending_channel_tests.rs. The client keeps cancellation ownership until a returned Channel takes its receiver. Dropping an unfinished open closes the receiver. If a confirmation was queued or already consumed, its exact channel ID is closed through the existing priority queue. Late confirmations close directly when delivery fails or the receiver is closed after delivery; the post-send check covers a send permit reserved before cancellation. No guessed IDs or scan of sibling channels is needed. Normal successful opens transfer the receiver out of the guard.

This addresses both cancellation before confirmation and cancellation after confirmation was queued but before the caller polled it. It creates no cleanup task and does not retain Yuzora's SSH handle mutex. Requests that a peer never confirms remain pending protocol requests until the SSH session ends; the patch does not claim to eliminate that separate condition.

Regression coverage lives in src-tauri/src/ssh_tcp_open_tests.rs and uses only owned loopback SSH/TCP fixtures with a temporary known-hosts file. Run individual exact tests after auditing them; the manual performance test compares normal 4 KiB/64 KiB transfers and both cancellation orderings. Evidence and the upstream file manifest live in .yuuzu/eval/performance-loop-2026-10-02/experiments/038-ssh-pending-tcp-open/.

Rejected channel opens report their error on the per-channel receiver and the
existing handler callback. Yuzora removes the duplicate, unconsumed
`Reply::ChannelOpenFailure` authentication notification and its private variant.
Repeated rejected opens otherwise accumulate this notification for the life of
the SSH handle. `src/client/rejected_reply_tests.rs` covers caller errors,
callbacks, connection health and the authentication queue without draining it.
The App Host/SFTP failure-path benchmark is in
`src-tauri/src/ssh_rejected_open_tests.rs`; experiment 039 records its evidence.

When upgrading russh, explicitly carry or replace these patches and rerun both cancellation orderings plus normal channel, repeated rejection and disconnect checks. A successful dependency build alone does not prove lifetime safety. Remove the local override only when equivalent behavior is verified in the replacement.
