# Windows-native Herdr WSL Agent Bridge

**Status:** Approved design

**Date:** 2026-08-23

**Scope:** Herdr upstream bridge capability plus truthful Yuzora Agent projection
**Supersedes:** the Yuzora WSL-native Herdr Runtime Provider direction

## Summary

Windows Yuzora will use Windows-native Herdr only. Users may enter WSL from any Windows Herdr managed pane and run Linux Pi, Claude Code, or Codex. Explicitly installed WSL adapters will report the foreground Agent back to the owning Windows Herdr pane through WSLInterop and Herdr's public CLI.

The bridge does not recreate a WSL Runtime Environment in Yuzora. Herdr remains the sole owner of Spaces, tabs, panes, terminal sessions, Agent identity, session identity, and state authority. Yuzora only projects the Agent records returned by Herdr snapshot and events.

The first release supports a reusable adapter interface with three adapters:

- **Pi:** Agent identity, native session identity, and authoritative lifecycle state.
- **Claude Code:** Agent identity and native session identity; `idle`, `working`, and `blocked` remain owned by Herdr's Claude screen manifest.
- **Codex:** Agent identity and native session identity; `idle`, `working`, and `blocked` remain owned by Herdr's Codex screen manifest.

## Goals

1. A Pi, Claude, or Codex TUI launched inside WSL from a Windows Herdr pane appears automatically as an Agent in Herdr and Yuzora after its adapter has been explicitly installed for that distro.
2. Preserve Herdr's existing state-authority semantics instead of reporting a synthetic `unknown` state.
3. Support both:
   - entering an interactive `wsl.exe` shell and launching the Agent later;
   - launching an Agent directly with `wsl.exe -d <distro> -- <agent>`.
4. Keep all reporting fail-open: bridge failure must never prevent or terminate an Agent.
5. Use public Herdr CLI and schema methods only. Do not use private transport protocols.
6. Require explicit per-distro, per-integration installation, update, and removal.
7. Allow WSL bridge adapters and ordinary WSL-native Linux Herdr integrations to coexist.
8. Preserve Yuzora's native-only Herdr architecture and its existing local Terminal WSL profiles.

## Non-goals

- Reintroducing a Yuzora WSL Herdr Runtime Provider.
- Running a Herdr server or sidecar inside WSL for this bridge.
- Creating a Windows named-pipe to WSL Unix-socket proxy.
- Running a global WSL daemon or process scanner.
- Detecting Agents by parsing terminal text in Yuzora.
- Supporting nested tmux or GNU screen in the first bridge release.
- Supporting every Herdr integration in the first release.
- Making Windows Git, worktrees, or workspace paths WSL-native.
- Automatically installing adapters without explicit user action.

## Domain model

### Host Runtime

The local operating-system environment that owns the Herdr server, PTYs, and observable process tree. Windows Yuzora uses one Windows Host Runtime.

### WSL Shell Profile

A shell workload launched through `wsl.exe` from a Windows pane. It is not a separate Herdr Runtime Environment and does not alter Herdr session or resource identity.

### Agent Projection

Yuzora's truthful display of Agent identity, session, state, and presentation fields returned by Herdr. Yuzora does not author Agent facts.

### Agent Identity Lease

A time-bounded claim that a specific Herdr pane currently hosts a known Agent kind. It identifies the Agent but does not author semantic state or session identity.

### Agent Execution Origin

Optional presentation metadata describing where the Agent process executes. The first non-native origin is `{ kind: "wsl", distribution }`. It never participates in resource identity or routing.

## Architecture

```text
Windows-native Herdr managed pane
│
├── pane-scoped HERDR_* + WSLENV contract
│
└── wsl.exe
    └── WSL shell
        └── Pi / Claude / Codex
            └── WSL Agent Adapter
                └── shared Bridge Runner
                    └── WSLInterop executes exact Windows herdr.exe
                        └── public pane report-* CLI
                            └── Windows Herdr named-pipe API
                                └── snapshot / events
                                    └── Yuzora Agents projection
```

The design contains five modules.

### 1. Windows Herdr WSL Bridge Manager

Responsibilities:

- install, update, inspect, and remove WSL adapters;
- maintain the list of explicitly enabled distro/integration pairs;
- inject the pane-scoped bridge environment into new managed panes;
- generate and validate pane-generation bridge capabilities;
- expose bounded diagnostics without exposing secrets.

### 2. WSL Bridge Runner

A shared, versioned, architecture-independent helper installed into each authorized distro. It:

- validates the bridge environment;
- accepts only a closed command map;
- acquires and renews Agent identity leases;
- reports session identity and Pi lifecycle state;
- invokes only the exact translated Windows `herdr.exe` path through WSLInterop;
- launches at most one per-Agent lease keeper;
- never opens a network listener.

### 3. Agent Adapters

The first release includes Pi, Claude, and Codex adapters. Adapters translate official Agent hook/extension events into the Bridge Runner's typed actions. They do not implement Windows IPC or construct arbitrary Herdr commands.

### 4. Herdr Agent Identity Lease

Herdr stores Agent identity independently from Agent session and state authority. A leased identity can select a screen manifest even when Windows process detection sees only `wsl.exe`.

### 5. Yuzora Agent Projection

Yuzora accepts the optional execution-origin field and displays a badge. It does not add WSL target keys, caches, page paths, connectors, or detection logic.

## User interface and commands

Adapter lifecycle is explicit:

```powershell
herdr integration install pi     --wsl Ubuntu
herdr integration install claude --wsl Ubuntu
herdr integration install codex  --wsl Ubuntu

herdr integration status         --wsl Ubuntu

herdr integration uninstall pi     --wsl Ubuntu
herdr integration uninstall claude --wsl Ubuntu
herdr integration uninstall codex  --wsl Ubuntu
```

Rules:

- `--wsl` accepts one installed distro name as an independent `wsl.exe --distribution` argument.
- Agent target is a closed enum.
- The command never starts an Agent or Herdr session.
- No adapter is installed as a side effect of detection.
- Installing one distro must not modify another distro.

## Pane environment contract

Windows Herdr merges the following entries into the inherited `WSLENV` for managed panes:

```text
HERDR_ENV/u
HERDR_PANE_ID/u
HERDR_WORKSPACE_ID/u
HERDR_TAB_ID/u
HERDR_SESSION/u
HERDR_BIN_PATH/pu
HERDR_WSL_BRIDGE/u
HERDR_WSL_BRIDGE_PROTOCOL/u
HERDR_WSL_BRIDGE_CAPABILITY/u
```

Values:

```text
HERDR_WSL_BRIDGE=1
HERDR_WSL_BRIDGE_PROTOCOL=1
HERDR_WSL_BRIDGE_CAPABILITY=<256-bit random value>
```

Contract rules:

1. Inject this contract only when Herdr's Windows state records at least one explicitly installed WSL adapter. Uninstalling the final adapter disables injection for newly spawned panes; existing pane generations retain their current environment until respawn.
2. Preserve unknown user `WSLENV` entries byte-for-byte and in order.
3. Deduplicate Herdr-owned variable names and replace their flags with the exact required flags.
4. `/u` limits initial propagation to Win32 → WSL.
5. `/pu` translates the absolute Windows `HERDR_BIN_PATH` to a WSL path and limits initial propagation to Win32 → WSL.
6. Do not propagate `HERDR_SOCKET_PATH`; the Windows named pipe is not a Linux Unix socket.
7. Do not interpolate environment values into shell source.
8. Generate a new capability on every pane generation or respawn.
9. Never persist the capability or include it in snapshots, events, logs, diagnostics, or error messages.

### Capability transport correction

The capability must not appear in process argv. The Bridge Runner copies it into a child-only `HERDR_BRIDGE_AUTH` environment variable, adds `HERDR_BRIDGE_AUTH/w` to the Windows-bound child `WSLENV`, unsets the WSL-facing capability for that child, and then directly executes the translated Windows binary. The Windows Herdr CLI reads `HERDR_BRIDGE_AUTH`, includes it in the local API request, and immediately clears its local copy.

Sources beginning with `herdr:wsl:` require valid bridge authentication. Ordinary same-host integration sources preserve their existing trust behavior.

## Public Herdr interface

### CLI

```text
herdr pane report-agent-identity <pane-id>
  --source herdr:wsl:<adapter>
  --agent <pi|claude|codex>
  --lease-id <uuid>
  --ttl-ms 30000
  --seq <u64>
  --execution-origin wsl
  --distribution <validated-name>

herdr pane release-agent-identity <pane-id>
  --source herdr:wsl:<adapter>
  --lease-id <uuid>
  --seq <u64>
```

Existing commands remain the session and state interfaces:

```text
herdr pane report-agent-session ...
herdr pane report-agent ...
herdr pane release-agent ...
```

For `herdr:wsl:*` sources, the CLI forwards bridge authentication from `HERDR_BRIDGE_AUTH`; no secret CLI option exists.

### Public schema methods

```text
pane.report_agent_identity
pane.release_agent_identity
```

The methods are additive, appear in the advertised schema, and are unavailable to clients whose server schema does not include them. Yuzora does not call these methods directly.

### Identity lease fields

```text
pane_id        required existing pane identity
source         stable adapter source
agent          known Agent enum
lease_id       random identity for one Agent process generation
ttl_ms         bounded; first release requires 30000
seq            strictly increasing within source + lease_id
origin         optional execution-origin presentation metadata
bridge_auth    sensitive request field, required for herdr:wsl:* sources
```

## Authority model

Herdr maintains independent authorities:

| Authority | Meaning | WSL source |
|---|---|---|
| Identity | Which known Agent occupies the pane | Agent identity lease |
| Session | Agent-native resumable session reference | Adapter session hook |
| State | `idle`, `working`, `blocked`, `done`, `unknown` | Pi lifecycle or screen manifest |

### Pi

```text
Identity  = WSL Pi Adapter lease
Session   = Pi extension
State     = Pi lifecycle extension
```

An active Pi lifecycle authority suppresses Pi screen-manifest fallback under existing Herdr rules.

### Claude Code

```text
Identity  = WSL Claude Adapter lease
Session   = Claude SessionStart hook
State     = Herdr Claude screen manifest
```

### Codex

```text
Identity  = WSL Codex Adapter lease
Session   = Codex SessionStart hook
State     = Herdr Codex screen manifest
```

The identity lease makes the Agent kind known even though Windows process detection sees `wsl.exe`. Herdr then evaluates the corresponding existing bottom-buffer screen manifest. Known-Agent fallback behavior remains unchanged: if no screen rule matches, state falls back to `idle`; strict visible approval/question evidence is required for `blocked`.

An identity lease must never create lifecycle authority or write `unknown` as a substitute state.

## Lease state machine

```text
Adapter starts
    │
    ├── validate foreground Agent process
    ├── immediate identity report
    ├── TTL = 30 seconds
    └── renew every 10 seconds
            │
            ├── normal exit → best-effort release
            ├── crash/kill  → renewal stops → TTL expiry
            ├── background  → release
            └── bridge fail → renewal stops; Agent continues
```

Rules:

1. `source + lease_id` identifies one lease.
2. A renew requires a strictly newer sequence.
3. Stale, duplicate, or out-of-order reports are ignored.
4. The same Agent may acquire a new lease only after the previous lease releases or expires.
5. A different live Agent lease conflicts and is rejected; there is no last-writer-wins takeover.
6. Pane close immediately clears all leases and related WSL presentation metadata.
7. Pane respawn invalidates the old capability and all old-generation reports.
8. After sleep/resume, the keeper may report the same lease again; valid authenticated reporting reacquires an expired lease.
9. Windows cold restart does not rotate capabilities into a surviving process environment. Windows live server handoff is outside this design: restored sessions respawn panes and receive new capabilities; any unexpectedly surviving old pane remains unbridged until respawn.
10. A conflicting new identity clears incompatible session references and presentation metadata only after the old lease is no longer live.
11. Server deadlines use monotonic time.

## Foreground-process proof

The bridge supports one foreground Agent per Herdr pane.

Before acquire and before each renewal, the keeper verifies:

- Agent root PID still exists;
- process start time matches the recorded value;
- the process has a controlling TTY;
- the Agent process group equals that TTY's foreground process group;
- adapter-specific bounded process identification remains valid;
- `TMUX` and GNU screen nesting are not active.

The adapter scans at most 16 ancestors using `/proc/<pid>/exe`, `/proc/<pid>/cmdline`, parent PID, process group, and process start time. It uses Herdr's existing known executable/wrapper rules. Missing or ambiguous identity fails closed and does not establish a lease.

When shell control returns, the keeper releases the lease. Backgrounded Agents do not remain the effective pane Agent.

## Installation model

Windows Herdr invokes WSL without dynamic shell source:

```text
wsl.exe
  --distribution <distro-argv>
  --exec /bin/sh -s -- <closed-target> <adapter-version>
```

A constant embedded installer is supplied on stdin. Dynamic distro, target, version, paths, and user values never become installer source text.

Installed layout:

```text
~/.local/lib/herdr-wsl-bridge/
├── bridge-runner
└── manifest.json

~/.pi/agent/extensions/herdr-wsl-agent-state.ts
~/.claude/hooks/herdr-wsl-agent-state.sh
~/.codex/herdr-wsl-agent-state.sh
```

Claude and Codex receive exact Herdr-owned config entries in their existing settings/hooks files.

Ownership definitions:

- **Claude:** modify `~/.claude/settings.json` only under `hooks.SessionStart`. Add one command hook whose command is exactly `bash '<absolute ~/.claude/hooks/herdr-wsl-agent-state.sh>' session`, with a 10-second Agent hook timeout. Installation first removes only entries whose command resolves to that exact Herdr-owned WSL hook path. Uninstall applies the same exact-path match and leaves all other SessionStart groups and commands unchanged.
- **Codex:** modify `~/.codex/hooks.json` only under `hooks.SessionStart`. Add one command hook whose command is exactly `bash '<absolute ~/.codex/herdr-wsl-agent-state.sh>' session`, with a 10-second Agent hook timeout. Installation and uninstall match only that exact Herdr-owned WSL hook path and `session` action. Ensure `[features] hooks = true` in `~/.codex/config.toml`; uninstall leaves that feature flag unchanged, matching Herdr's existing Codex integration ownership policy.
- Adapter markers are exact and start at version `1`: Pi uses `HERDR_INTEGRATION_ID=wsl-pi`, Claude uses `HERDR_INTEGRATION_ID=wsl-claude`, and Codex uses `HERDR_INTEGRATION_ID=wsl-codex`; each uses `HERDR_INTEGRATION_VERSION=1`. Status requires both the current file marker and the exact managed config entry.

Installer requirements:

- Agent configuration directory must already exist.
- Writes use temporary files and atomic rename.
- JSON/TOML must parse before modification.
- Symlinks, non-regular files, unsafe ownership, and unsafe parent permissions fail closed.
- Partial failure rolls back files and config entries created by that invocation.
- Repeated install is idempotent.
- Update replaces only Herdr-owned adapter assets and entries.
- Uninstall removes only exact Herdr-owned files and entries.
- `manifest.json` records bridge protocol `1` and adapter versions.
- `status` verifies asset version, managed config entries, WSLInterop availability, and last bounded local error category.

## Coexistence with Linux Herdr

WSL bridge adapters and native Linux Herdr adapters use different filenames and activation gates.

```text
Linux Herdr integration
└── requires HERDR_SOCKET_PATH

Windows → WSL Bridge adapter
└── requires HERDR_WSL_BRIDGE=1
    + HERDR_WSL_BRIDGE_CAPABILITY
```

Windows Herdr does not propagate `HERDR_SOCKET_PATH`, so ordinary Linux Herdr hooks no-op in the bridge path. Linux Herdr does not inject the bridge variables, so WSL bridge adapters no-op in the WSL-native Herdr path.

Both sets may be installed simultaneously without sharing an authority source.

## Bridge Runner

The runner accepts only:

```text
acquire-identity
report-session
report-state      # Pi only
release
status-local
```

Security and behavior:

- `agent` is limited to `pi`, `claude`, or `codex`.
- Pane ID, Herdr session, binary path, and capability come only from environment.
- Hook payload cannot override authority or routing values.
- `HERDR_BIN_PATH` must be absolute, translated, and end in `.exe`.
- WSLInterop must be enabled; there is no fallback to a Linux `herdr` on `PATH`.
- Invocation uses direct argv and no `eval`, command-string execution, or user-derived shell source.
- Reporter workers close inherited stdio before detaching.
- One-shot reports use a 500 ms attempt and at most one 1500 ms retry.
- Every failure preserves the Agent's original exit code.
- No command starts or stops WSL, Herdr, a pane, or an Agent.

### Lease keeper

The runner starts one detached child per active foreground Agent. It is not a global daemon.

Deduplication key:

```text
pane_id + agent + root_pid + process_start_time
```

The keeper:

- reports immediately;
- renews every 10 seconds;
- validates foreground ownership on every renewal;
- best-effort releases on process exit or foreground loss;
- exits after release or unrecoverable environment failure;
- never launches a new WSL instance;
- uses bounded local lock/state files that reject symlinks and stale PID reuse.

## Adapter behavior

### Pi Adapter

Installation path:

```text
~/.pi/agent/extensions/herdr-wsl-agent-state.ts
```

Behavior:

- activates only for root TUI sessions;
- ignores RPC, print, and headless modes;
- starts the keeper using the Pi process PID;
- reports Agent session ID/path;
- maps `agent_start` to `working`;
- maps Pi's existing local extension-bus `herdr:blocked` event to `blocked` with a bounded message; this event is produced inside Pi and is not delivered from the Windows Herdr server;
- maps settled idle state to `idle`;
- does not let subagents replace root pane identity;
- preserves report ordering with monotonically increasing sequence numbers.

The Pi Adapter is outbound-only. It subscribes to Pi lifecycle and local extension events in-process; the Bridge Runner and Windows Herdr do not send events back into WSL.

### Claude Adapter

Installation path:

```text
~/.claude/hooks/herdr-wsl-agent-state.sh
```

Behavior:

- installs an exact managed `SessionStart` hook entry;
- ignores subagent payloads and `SubagentStop`;
- discovers one unambiguous Claude root process through the bounded ancestor scan;
- starts the identity keeper;
- reports session ID and transcript path;
- never reports semantic state.

### Codex Adapter

Installation path:

```text
~/.codex/herdr-wsl-agent-state.sh
```

Behavior:

- installs an exact managed `SessionStart` hook entry and enables Codex hooks;
- requires session ID and transcript path;
- rejects a conflicting inherited `CODEX_THREAD_ID`;
- discovers one unambiguous Codex root process through the bounded ancestor scan;
- starts the identity keeper;
- reports session ID;
- never reports semantic state.

## Agent execution origin

Herdr Agent snapshot/events gain an optional field:

```json
{
  "execution_origin": {
    "kind": "wsl",
    "distribution": "Ubuntu"
  }
}
```

The runner obtains `distribution` only from WSL's `WSL_DISTRO_NAME`, not hook payload. Herdr validates maximum length, UTF-8, control characters, and output bounds.

The field is presentation-only. It must not affect:

- named-session identity;
- workspace, tab, pane, or terminal identity;
- page paths or cache keys;
- connector routing;
- mutation authority;
- Git or filesystem path interpretation.

## Yuzora behavior

Yuzora continues to render only Herdr Agent records.

Display rules:

```text
execution_origin absent/native → no badge
kind = wsl + distro            → WSL · <distribution>
kind = wsl without distro      → WSL
```

Yuzora must:

- treat the new field as optional and forward-compatible;
- apply text/control-character bounds before rendering;
- avoid adding RuntimeTarget or distro to page/resource identity;
- never inspect `wsl.exe`, WSL processes, terminal screen text, or hook files;
- never synthesize an Agent when Herdr reports none.

## Error handling

| Failure | Required behavior |
|---|---|
| Adapter not installed | Agent runs normally and is not bridged |
| WSLInterop unavailable | Runner exits successfully without fallback |
| Invalid `HERDR_BIN_PATH` | No PATH lookup and no report |
| Missing/invalid capability | Herdr rejects without altering authority |
| Windows Herdr unavailable | Bounded timeout; Agent continues |
| Keeper crash | Lease expires within the TTL |
| Agent crash or distro termination | PID disappears or lease expires |
| Sleep/resume | Keeper reacquires with authenticated report |
| Herdr cold restart | Old reports become invalid; Windows session restore respawns the pane with a new capability. No surviving-process capability rotation is claimed |
| Screen manifest has no match | Existing known-Agent idle fallback applies |
| Nested tmux/screen | Adapter does not acquire; diagnostic reason is recorded |
| Identity conflict | Existing live identity remains authoritative |

Bridge errors must not terminate the Agent, pane, WSL distro, or Herdr server.

## Diagnostics and observability

### Windows Herdr

`herdr pane info <pane>` exposes:

- identity source;
- Agent kind;
- execution origin;
- lease age and expiry;
- last accepted sequence;
- last bounded rejection category.

### WSL integration status

`herdr integration status --wsl <distro>` exposes:

- Bridge protocol and installed version;
- WSLInterop availability;
- Pi/Claude/Codex adapter state;
- exact managed-entry status;
- last bounded local error category.

### Logging requirements

- Never log or serialize capabilities.
- Do not log every successful renewal.
- Redact paths and hook payloads under existing Herdr diagnostic policy.
- Rate-limit rejection logs by pane/source/category.
- Keep active leases, sources, strings, and metadata under hard server bounds.
- Do not retain prompts or terminal contents in Bridge diagnostics.

## Security invariants

1. Dynamic distro, target, path, session, PID, and payload values never enter executable shell source.
2. Adapter target and runner actions are closed enums.
3. Hook payload cannot choose pane ID, Herdr binary, session routing, source, or capability.
4. A WSL report cannot affect a pane generation whose capability it does not possess.
5. Capability is absent from argv and all persistence/diagnostic surfaces.
6. Config editing is ownership-scoped, transactional, and symlink-safe.
7. A Linux `herdr` on `PATH` cannot receive bridge reports.
8. The Bridge opens no listener and accepts no remote traffic.
9. PID identity includes process start time to prevent reuse.
10. A background or ambiguous Agent cannot remain the effective pane Agent.
11. Processes within one managed pane's descendant tree share that pane's trust domain and may read its capability. The capability prevents cross-pane and stale-generation reporting; it is not a sandbox between descendants of the same pane.

## Performance budgets

| Metric | Budget |
|---|---:|
| Adapter hook synchronous return | `< 100 ms` p95 |
| First identity visible in Herdr | `< 2 s` p95 |
| Lease renewal interval | `10 s` |
| Crash-to-expiry | `≤ 32 s` |
| Lease keepers per active Agent | exactly `1` |
| CLI attempts per one-shot/renewal | maximum `2` |
| Additional network listeners | `0` |
| Idle report rate per Agent | `≤ 0.12 req/s` average |

Adapters detach reporter workers before returning so a full CLI timeout cannot delay Agent startup.

## Automated verification

### Identity Lease

- report, renew, release, and TTL expiry;
- duplicate, stale, and out-of-order sequence handling;
- invalid capability and wrong pane generation;
- identity conflicts and expiry/replacement;
- pane-close and respawn cleanup;
- identity does not create lifecycle authority;
- leased Claude/Codex identity selects the correct screen manifest;
- Pi lifecycle authority remains stronger than screen detection;
- snapshot/event optional-origin compatibility;
- capability redaction across all outputs.

### WSLENV and launcher

- merge preserves unknown entries and ordering;
- Herdr-owned entries deduplicate to exact flags;
- paths containing spaces, Unicode, parentheses, and shell metacharacters;
- hostile distro names remain one argv value;
- no dynamic `sh -c` source;
- WSLInterop unavailable;
- invalid translated executable path;
- named-pipe timeout;
- isolation across multiple distros.

### Installer

For each adapter:

- fresh install;
- idempotent reinstall;
- version update;
- exact status detection;
- ownership-scoped uninstall;
- unrelated hooks remain byte-equivalent;
- malformed JSON/TOML fails closed;
- symlink/non-regular/unsafe-owner rejection;
- partial-write rollback;
- Linux Herdr integration coexistence;
- one distro cannot modify another.

### Runtime

- immediate report and one keeper;
- PID start-time reuse protection;
- foreground process-group checks;
- shell foreground regain and release;
- crash, `kill -9`, distro termination, and TTL;
- sleep/resume reacquisition;
- duplicate hook-event deduplication;
- nested tmux/screen fail-closed behavior;
- Pi root/subagent isolation and Pi-local `herdr:blocked` event mapping without an inbound Herdr listener;
- Claude subagent exclusion;
- Codex thread conflict rejection;
- hook failure preserves Agent exit status.

### Yuzora

- native/absent origin produces no badge;
- WSL distro badge rendering;
- missing/hostile/overlong distro handling;
- origin cannot change resource identity or connector routing;
- unknown optional fields remain compatible;
- no Yuzora-side Agent fallback exists.

## Windows + WSL2 manual acceptance

Test on Windows 11 with at least two WSL2 distributions.

1. Run Agents before adapter installation; they must remain usable and unbridged.
2. Explicitly install each Pi, Claude, and Codex adapter.
3. Enter interactive `wsl.exe` from a Windows Herdr pane and launch each Agent.
4. Launch each Agent directly with `wsl.exe -d <distro> -- <agent>`.
5. Verify identity, native session, and the intended state authority.
6. Exercise `Ctrl+Z`, shell return, normal exit, `kill -9`, and `wsl --terminate`.
7. Exercise sleep/resume; then cold-restart Herdr and verify session restore respawns the pane, re-runs the adapter, and uses a new capability.
8. Run simultaneous Windows-native and WSL Agents in different Herdr panes.
9. Install ordinary Linux Herdr integrations in the same distro and verify activation isolation.
10. Uninstall adapters and verify exact cleanup.
11. Close Yuzora and verify it does not stop Herdr, WSL, panes, or Agents.

Collect Herdr diagnostics, Yuzora Agent screenshots, pane info, integration status, process trees, versions, and hashes.

## Delivery and rollout

```text
H1 — Herdr Identity Lease
├── public methods and CLI
├── authority separation
├── lease/capability state machine
└── execution-origin DTO

H2 — Herdr WSL Bridge
├── pane WSLENV contract
├── install/status/uninstall
├── Bridge Runner
├── Pi/Claude/Codex adapters
└── foreground keeper

Y1 — Yuzora Projection
├── optional DTO normalization
├── WSL badge
└── no detection/routing changes
```

Dependency order is `H1 → H2 → Y1`.

Rollout rules:

- Ship H1/H2 through Herdr's Windows preview channel first.
- Advertise all new methods in the public schema.
- Bridge protocol starts at version `1`; every adapter has an independently checkable version.
- Runtime behavior is capability/schema-gated, not hard-coded to a product version string.
- Yuzora enables the badge only when Herdr supplies `execution_origin`.
- Do not claim complete Windows-to-WSL Agent support before the manual matrix passes.
- Never reintroduce a Yuzora WSL Runtime Provider as part of this delivery.

## Completion criteria

The first release is complete only when all are true:

- Pi has identity, native session, and accurate lifecycle state.
- Claude has identity, native session, and existing screen-manifest state.
- Codex has identity, native session, and existing screen-manifest state.
- Any managed-pane `wsl.exe` invocation can carry the contract.
- Every distro/integration requires explicit installation.
- All failures remain fail-open for the Agent.
- No socket proxy or global daemon exists.
- Yuzora performs truthful projection only.
- Automated security and lifecycle tests pass.
- Windows + WSL2 manual acceptance passes.
