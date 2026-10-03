# SDD ledger — plan: docs/superpowers/plans/2026-10-02-hangar-server-conversas.md

Spec: docs/superpowers/specs/2026-10-02-hangar-server-conversas-design.md
Worktree: .claude/worktrees/hangar-server-parte1, branch hangar-server-parte1 (from main d78a3eb0).
User authorized (2026-10-02): run each Task's own tests (cargo test of touched crate, pytest of touched test files); never the whole suite, never start/restart the live backend.
Pre-flight table: preflight.md (95 rows, 18 conflicts).

## Pre-flight rulings
- Ruling: P1 T12 uses `gen` (reserved in edition 2024) — rename to `generation` everywhere in T12 — cost if wrong: none.
- Ruling: P2 Cargo.lock stale — every Rust Task runs `cargo check` (in crates/) before commit so the committed crates/Cargo.lock matches; CI `--locked` must pass — cost: none.
- Ruling: P3 T8 inserts its Codex test between T7's `#[test]` and fn — insert after the whole T7 test fn instead — cost: none.
- Ruling: P4 T9 "trocar a primeira linha" of claude.rs — replace the `//!` doc line, keep the path comment — cost: none.
- Ruling: P5 T3 native.yml paths anchor is lines 9-13; keep `workflow_dispatch:` — cost: none.
- Ruling: P6 T13 CP_LAN_BIND_IP=localhost — Python resolves hostnames to an IP literal before building HANGAR_SERVER_LISTEN — cost: none.
- Ruling: P7 T13 Settings field `rust_server` — keep field (works from .env); add DESCRICAO/messages only if an existing guard test requires it; no UI edits (spec asks no toggle; web desktop frozen) — cost if wrong: setting not visible in config screens.
- Ruling: P8 T15 MainPID is `uv` — measure the python child (`pgrep -P <MainPID>`) and the hangar-server child — cost: none.
- Ruling: P9 T14 fetch() must never raise — catch OSError, http.client.HTTPException, ValueError around download+hash — cost: none.
- Ruling: P10 T14 atualizar.py import anchor is line 44 — cost: none.
- Ruling: P11 T12 side-events read error log must not include `{e}` body text — log error kind/status only — cost: none.
- Ruling: P12 T11 X-Forwarded-For non-ASCII/unparseable → use the TCP peer address, never default to 127.0.0.1 — cost: none (security).
- Ruling: P13 RewriteFilter fingerprint ensure_ascii — T7's choice stands (internal md5, same comparison outcome); Global Constraint/spec wording is imprecise, not the code — cost if wrong: rewrite suppression mismatch vs Python, caught by golden tests.
- Ruling: P14 offset > file size → T12 keeps Python's current behavior (last 200 lines); spec wording yields to "clients don't change" — cost: none.
- Ruling: P15 T12 three-device reset test — use 3 clients, assert exactly one reset each and one internal reconnect — cost: none.
- Ruling: P16 T5 test count 21 vs 22 — report actual count — cost: none.
- Ruling: P17 T2 lock diff path — run git diff from repo root — cost: none.
- Ruling: P18 T10 internal route access test — loopback client address, assert 200 with secret and 404 without — cost: none.
- Ruling: D29 new identifiers in Portuguese in several Tasks — new identifiers in English (CLAUDE.md rule); existing names stay — cost: none.

## Tasks
Task 1: complete (commits d78a3eb0..b6b15767, review clean)
Task 1: minor (deferred): contract.rs round_trip compares Value (order-insensitive); key order of structs not proven vs model_dump_json
Task 1: minor (deferred): rustfmt not installed in toolchain; long lines unformatted
Task 2: complete (commits b6b15767..c7a8645c, review clean)
Task 2: minor (deferred): limit expression duplicated app.rs:5295 / side.rs:693 untested (plan-mandated duplication pre-existed)
Task 2: minor (deferred): orq_entry() re-deserializes Map per frame; no test for unreadable orq -> None
Task 2: note: app::sidebar::tests::waiting_on_top_then_by_name_and_never_twice fails before and after (pre-existing, not touched)
Task 2: note: real-use Step 7 deferred to Task 15 manual verification
Task 3: complete (commits c7a8645c..61f5257c, review clean)
Task 3: minor (deferred): server.yml trigger backend/tests/fixtures/** broader than needed (could be fixtures/contract/**)
Task 3: check-later: server.yml cano contract step only meaningful after Task 5 reads CP_RUST_CANO_BIN; server-latest publishes stubs until Tasks 4/11 — push ordering
Task 4: complete (commits 61f5257c..a1d0633a, review clean)
Task 4: minor (deferred): main.rs:512 child.id().unwrap_or(0) then kill(0) would signal own group — unreachable today
Task 4: minor (deferred): --token "" semantics differ from Python (unreachable: backend always passes non-empty)
Task 4: check-later (Task 15 VM): Windows spawn of hangar-engine.CMD via Rust std batch escaping; Windows termination
Task 4: note: lifecycle/concurrency paths covered by Task 5 parametrized pytest against the binary
Task 5: complete (commits a1d0633a..4abc27c6, review clean)
Task 5: minor (deferred): key-in-cmdline assert may be satisfied by socket path, not specifically --log
Task 5: minor (deferred): binary present but unexecutable (exec format) has no fallback to cano.py
Task 5: check-later (Task 15): Step 10 real session on hangar-cano (cmdline, restart with pending card, close, fallback)
Task 6: complete (commits 4abc27c6..aca247b7, review clean)
Ruling: T6 naive (no-timezone) ISO timestamps read as UTC in Rust vs local time in Python RewriteFilter/merged_history — accepted (plan-listed): real Claude/Codex transcripts always carry Z/offset; golden pins TZ=UTC — cost if wrong: ordering/rewrite window off by the UTC offset for a hypothetical naive-timestamp transcript
Task 6: minor (deferred): integer -0 becomes -0.0 (Python 0) — would change a Codex id sha1; improbable
Task 6: minor (deferred): serde_json nesting limit 128 vs Python ~1000 — deep tool_input line skipped in Rust
Task 6: minor (deferred): dumps_unicode/loads_lossless pub return private-use marker; untested scrub_map key collision, strip \x1c-\x1f, Provider::parse/as_str
Ruling: T7 review Important (plan-mandated) peer_name — native cross-session message without [de:] prefix must show the tmux session name like Python (registry.name_of_pid: tmux list-panes -a pane pids + /proc descendants, cache keyed by pid+start time, failure → None not cached) — port it to Rust (Linux /proc; other OS → None → title fallback, documented); LineParser gets an injectable resolver so goldens keep None — cost if wrong: one tmux list-panes per unseen peer pid on the parse path
Task 7: fix round 1/5 (1 addressed, 0 open — peer label tmux name; commits ca5508f2..072c2127)
Task 7: complete (commits aca247b7..072c2127, review clean)
Task 7: minor (deferred): peer.rs list_panes polls try_wait without draining stdout (>64KB pane list would time out)
Task 7: minor (deferred): py repr prints private-use/unassigned chars raw; [\w-] class differs on exotic chars; negative cache_read -> None; no golden for from-name wrap / dict tool_result content
Task 7: carry-to-T12: LineParser::feed may fork tmux + scan /proc (peer name) — call it from spawn_blocking, never on the async runtime; feed only complete lines (rewind partial last line like transcript.py:800-805); RewriteFilter applies to claude AND claude-headless live, but /history only for provider=='claude' (pqueue.py:1040)
Task 8: complete (commits 072c2127..8755ffed, review clean; reviewer diffed 47 synthetic Codex lines vs Python: 0 differences incl. ids)
Task 8: minor (deferred): wrong-typed fields (numeric arguments, text null, non-string name) yield events in Rust where Python raises and kills the tail — Rust-favorable divergence
Task 8: minor (deferred): golden codex.jsonl lacks rejected/fulfilled/session_id/split header/\x41 cases (verified by reviewer, not pinned)
Task 8: minor (deferred): Codex line with NaN/Infinity or int > u64 dropped by Rust (inherited from T6)
Task 9: complete (commits 8755ffed..79164a30, review clean)
Task 9: minor (deferred): history.rs:98 partial_cmp unwrap_or(Equal) — hand-edited queue ts "nan" could make sort_by panic (fix: drop non-finite ts, keep prev_ts)
Task 9: minor (deferred): HistoryRequest{limit:Some(0)} built directly returns empty (only history_request filters n>0); doc 'transcript ausente dá vazio' imprecise (queue entries still return)
Task 9: minor (deferred): test_secret_never_goes_to_environ weak (check SECRET not in os.environ.values())
Task 9: carry-to-T12: history_request()==None → proxy to Python; missing transcript with queue still returns queue entries (not 404)
Task 10: complete (commits 79164a30..661a66a1, review clean)
Task 10: minor (deferred): side-events refusal (404) writes no diag.registrar like /events does (api.py:3356-3357) — fallback reason not in the diary
Task 10: minor (deferred): 'if side: continue' placed under an unrelated comment; test reads plugin_bridge._apps_abertos
Task 11: fix round 1/5 (1 addressed, 0 open — lockout parity with Python 403-upgrade/429; commits 7a75d30c..bcd257a0)
Task 11: complete (commits 661a66a1..bcd257a0, review clean)
Task 11: minor (deferred): config.rs #[derive(Debug)] on Config holding auth_token/internal_secret (future {:?} log would leak) — manual Debug
Task 11: minor (deferred): lib.rs init_log silently falls back to stderr when the log file can't open
Task 11: minor (deferred): legit owner WS 403 (CP_TERM_ORIGINS) also counts toward disabling the shortcut — safe side
Task 11: carry-to-T12: end-to-end check that a locked origin's owner request is proxied; maybe_gzip must refuse text/event-stream itself; proxy tests' info_calls()==0 need positive counterpart; remove #[allow(dead_code)] on gate/maybe_gzip
Ruling: T12 Important (plan-mandated) ask_question kept in the side snapshot reopens an answered question for late devices — fix: drop cached ask_question when a non-awaiting_input state is recorded for non-codex/headless (Python only emits on connect while awaiting) — cost if wrong: a pending question missed by a late device until next state change
Ruling: T12 Important (plan-mandated INFO_TTL) /history served from the 1 s info cache can show a recreated session's dead conversation with nothing correcting it — fix: /history calls fetch_info uncached; cache only for /events opens (corrected by reset) — cost: one internal call per history request
Ruling: T12 Minor-1 promoted into the fix round: orphan side connection after hub close keeps app=1 and silences push forever — ensure_current/restart_side no-op when bound is empty — cost: none
Task 12: fix round 1/5 (3 addressed, 0 open — stale ask_question in snapshot, /history uncached info, closed hub no side restart; commits af6b1a37..45de181c)
Task 12: complete (commits bcd257a0..45de181c, review clean)
Task 12: minor (deferred): D19 queue list in side snapshot only cleared on internal reconnect; two devices with new info at once may get two resets; Watchers::subscribe creates inotify thread on a tokio worker; Connection: keep-alive header untested; slow /history fetch_info may overwrite newer cached info (at most one extra reset)
Task 13: fix round 1/5 (1 addressed, 0 open — normal stop no longer counted as crash; commits 80b2e863..c7a85d8e)
Task 13: complete (commits 45de181c..c7a85d8e, review clean)
Task 13: minor (deferred): erro path (binary without +x) and stop() kill-after-5s untested; no liveness check after up (hung child keeps port); windows-tasks.ps1 child filter lacks CreationDate PID-reuse guard; rust_server description code has no phrase in UI maps (P7); grace-window timing untested
Task 13: check-later (Task 15 VM): windows-tasks.ps1 scenarios (pwsh absent); fallback rebind of 8765 on Windows; Stop-Process race
Ruling: T14 version skew (reviewer ⚠) — no commit comparison; the internal API stays versioned by INTERNAL_PROTOCOL/RUST_SERVER_PROTOCOL (any change to /internal routes or env contract must bump it — rule goes into Task 15 docs) and hangar-cano by snapshot versao; rust_release logs the manifest commit in the diário for diagnosis — cost if wrong: a Rust/Python mismatch without a protocol bump goes unnoticed until behavior differs
Ruling: T14 Minors 1 (_old_name unbounded loop → hang) and 3 (UnicodeEncodeError on Windows stdout fails the step despite --never-fail) promoted into fix round 1: both violate 'download never fails/hangs install/update' — cost: none
Task 14: fix round 1/5 (4 addressed, 0 open — download deadlines, bounded .old name, cp1252-safe output, commit in diário; commits 8bc4f286..c7024dc0)
Task 14: complete (commits c7a85d8e..c7024dc0, review clean)
Task 14: minor (deferred): worst case ~520 s vs 600 s step budget (limited margin); _old_name checks 99 not 100 candidates; mkdir failure logged as manifesto; failed Windows rollback not distinguished; leftover .tmp not swept; install.ps1 hides the failure reason
Task 14: check-later (Task 15 VM): install.ps1 parse (pwsh absent); real download from server-latest
Task 15: complete (commits c7024dc0..97071e4d, review clean)
Task 15: minor (deferred): plataforma.md points to the gitignored plan for the manual script (dangling in other checkouts); CLAUDE.md omits sem_resposta/erro triggers and doesn't say CP_RUST_SERVER_BIN wins without fallthrough; CLAUDE.md rule ~11 lines
Final review: With fixes — Critical 1 (hangar-cano needs GLIBC_2.39 from ubuntu-latest build; no fallback to cano.py → headless sessions fail on Debian 12/Ubuntu 22.04/RHEL 9), Important 2 (--resume rewrite duplicated live: shared tail LineParser starts empty at cut), 3 (proxy pool idle 90s vs uvicorn keep-alive 5s → sporadic 502 on POST), 4 (hangar-server.log unbounded). Must-fix minors: T15 dangling plan pointer; recommended: Config Debug redaction, tail panic → visible close.
Ruling: final fix wave includes Critical 1 (portable Linux build + one-time hangar-cano probe with fallback to cano.py and diário entry), Importants 2-4, T15 roadmap moved into the committed doc, Config Debug redaction, tail reader panic surfaced (catch_unwind or close) — the rest of deferred minors wait — cost if wrong: small extra diff
Final fix wave: 1 round (7 addressed, 0 open; commits 97071e4d..3aaae19b). Residual minors: probe lacks CREATE_NO_WINDOW on Windows (one console flash); musl allocator throughput -14% single / -57% at 8 concurrent big-history parses (fix: mimalloc or zigbuild glibc 2.28); log rotates only at startup; failed probe disables hangar-cano until restart; hangar-server has no panic hook (panic text in journal may include conversation text).
Plan steps marked [x] (122/127); open: T2 S7, T3 S5, T5 S10, T15 S4, T15 S5 (manual/post-push).
Owner choice (2026-10-02): keep musl + mimalloc (af306cdd); measured +13% (1 thread) / +17% (8 threads) vs gnu on 177 MB synthetic read_frames.
Owner choice (2026-10-02): glibc 2.28 via cargo-zigbuild instead of musl+mimalloc (52291add; review Approved; minor: stray blank line main.rs:2). Measured zigbuild vs gnu equal (1 thread 0.28 s/10 MB; 8 threads 0.62 s/45 MB vs 0.58 s/48 MB).
