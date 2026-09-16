# Fix plan — codeg issues #733 / #734

> **STATUS: IMPLEMENTED.** This document is the reviewed plan the two fixes were
> built from; it is kept for the reasoning and the review trail, and can be dropped
> before landing. The commits are `168117f6` (#734) and `24632320` (#733); what
> actually shipped, and where it differs from the plan, is in section 6.
>
> Revision 3, after two rounds of independent slice review. Changes vs revision 1
> are marked **[R2]**, vs revision 2 **[R3]**; see sections 4 and 5.

---

## 0. What the user asked for (this is the spec)

> 帮我评估这两个 issue（issue 内容仅供参考,你要自己理解里面的问题是否属实）,有问题就规划一个正确的修复方案
> https://github.com/xintaofei/codeg/issues/733
> https://github.com/xintaofei/codeg/issues/734

Follow-up: **检查方案是否达到生产可用级别** ("check whether the plan reaches
production-ready level").

Deliverable: (a) an independent judgement of whether each issue is real, and (b) a
fix plan a maintainer could hand to an implementer without further research.

---

## 1. Issue #734 — DeepSeek sessions written by `deepseek-acp` 0.9.0 render empty

### 1.1 Reporter's claim

Since `deepseek-acp` 0.9.0 (dsh stack `0.1.5-rc.1`), the session log is written as
`session.v3.jsonl.zstd`. codeg's parser only looks for `session.jsonl.zstd` /
`session.jsonl`, so post-upgrade conversations show zero turns and are missing from
the local-session list. The reporter further speculates that a **new record decoder**
is probably needed alongside the existing v0/v1 path.

### 1.2 Verdict: REAL. Severity P0.

### 1.3 Verified facts (established by command; treat as given)

| # | Fact | How established |
|---|---|---|
| F1 | `~/.dsh/sessions` on this machine holds **18** `session.jsonl.zstd` and **3** `session.v3.jsonl.zstd` | `find ~/.dsh/sessions -maxdepth 3 -type f -name 'session*'` |
| F2 | `--Users-xggz-work-my-app--/67a31fe6…/` holds **both** `session.jsonl.zstd` (mtime Sep 8) **and** `session.v3.jsonl.zstd` (mtime Sep 12) | `ls -la` |
| F3 | v3 header is `{"type":"session","version":3,"id","createdAt","cwd","isSeeded","delegationDepth"}`; v0 header is the same minus `isSeeded`, with `"version": 0` | `zstd -dc <file> \| head -1` on both |
| F4 | Upstream canonical name rule: `CANONICAL_LOG_FILENAME = /^session(?:\.v([1-9][0-9]*))?\.jsonl$/u`, applied **after** stripping the compression suffix. v0 keeps `session.jsonl`. `.v0`, leading zeros, uppercase and temp names are **not** canonical | `@deepseek-ai/dsh-session-format@0.1.5-rc.1`, `lib/index.js` |
| F5 | Upstream selection rule: *"Runtime operations select the numerically highest canonical generation"*; *"A root belongs to one encoding: startup discovery and targeted lookup reject generations with the other suffix"* | `@deepseek-ai/dsh-session-persistence-jsonl@0.1.5-rc.1` README lines 70 / 98 |
| F6 | A **write** open migrates the source and *publishes the successor without overwrite*; *"The source remains byte-identical"*, *"retained predecessors do not provide automatic fallback"* | same README line 80; corroborated by F2 |
| F7 | `@deepseek-ai/dsh-session-format` has exactly **one** published version; `deepseek-acp@0.8.0` depends on dsh `0.1.1-rc.2` ⇒ **v1/v2 never shipped as a current format**; only v0 and v3 can be on disk | `npm view` on both |
| F8 | A real v3 log contains: `session`, `agent/inbox/spliced`, `turn/start`, `step/start`, `system/message`, `user/message`, `session/title`, `request/header`, `request/context`, `assistant/message`, `tool/call`, `tool/result`, `step/end`, `turn/end`, `session/end-seed` | decoded the real file |
| F9 | In v3 the packed delta rows are gone; the compacted stream rides inside `assistant/message.data.stream`. `data.message.content[]` still carries `text`/`reasoning`/`tool-call{id,name,arguments}`; `data.usage` still carries `inputTokens`/`outputTokens`/`cacheReadTokens`/`reasoningTokens` | decoded the real file |
| F10 | In v3 a real prompt is still `user/message` with `data.source.kind == "user"`. Plumbing prompts use `kind:"plugin"` and a NEW `kind:"skill-catalog"`; codeg's guard is a whitelist so the new kind is still excluded | decoded the real file |
| F11 | `tool/result` gains `sourceEventSeqs` / `surfaceOp`, but `data.message.content[0]` is still a `tool-result` with `toolCallId`/`content[]`/`isError` | decoded the real file |
| F12 | `@deepseek-ai/dsh-compaction@0.1.5-rc.1` still emits `compaction/{start,summary,end,prune}` and still contains `compactionId`, `sourceCommandId`, `shadowedTokenCount` | `npm pack` + grep |
| F13 | codeg pins `deepseek-acp@0.9.0` as the built-in install (`src-tauri/src/acp/registry.rs:1639`, `:2134`) ⇒ every user on the default pin is affected | read the file |
| F14 | `regex = "1"` is already a dependency of `src-tauri` (`Cargo.toml:94`) | read the file |
| F15 **[R2]** | `read_session_log_text` is the **only** backend site whose behaviour depends on the log filename or generation. `resolve_fork_point` (`acp/fork.rs:102`) consumes already-parsed `MessageTurn`s and touches no filesystem; the backup source (`parsers/mod.rs:181-191`) archives the whole sessions root and is filename-agnostic | independent grep over `src-tauri/src/` + read of both files |
| F16 **[R2]** | `deepseek-acp` never configures the persistence `compression` option, so every generation it has ever written is zstd; the raw `.jsonl` branch exists only for a hypothetical `compression:'none'` deployment | grep of the agent tarball; corroborated by F1 (no raw file on disk) |

### 1.4 Root cause (verified by reading)

`src-tauri/src/parsers/deepseek.rs:348-356`

```rust
fn read_session_log_text(session_dir: &Path) -> Option<String> {
    let zstd_path = session_dir.join("session.jsonl.zstd");
    match fs::read(&zstd_path) {
        Ok(bytes) => decode_zstd_frames_prefix(&bytes),
        Err(_) => fs::read_to_string(session_dir.join("session.jsonl")).ok(),
    }
}
```

Both lookups miss on a v3-only directory ⇒ `parse_session_log` returns `None` ⇒

* `build_summary` (`:186`) returns `None` → the session vanishes from `list_conversations`;
* `build_detail` (`:219`) does `.unwrap_or_default()` → `turns == []`.

Folder conversations use the same parser (`commands/conversations.rs:1380`), which is
why `POST /api/get_folder_conversation_turns` answers `turns_total: 0`.

### 1.5 Second failure mode the issue does NOT mention

By F6 + F2: after a pre-0.9.0 session is reopened for write by 0.9.0, the directory
holds **both** generations and the v0 predecessor is frozen at the migration point.
codeg reads the predecessor, so the session does not look empty — it looks **silently
truncated at the migration timestamp**, with no error anywhere.

**This failure mode is the reason the resolver must never fall back to a lower
generation. [R2]**

### 1.6 Correction to the reporter's speculation

"A current-generation decoder probably has to be added" is **not true for codeg**:

* codeg never read the packed delta rows — `deepseek.rs:645-652` already skips
  `assistant/chunk` / `tool-call-chunks` / `reasoning-chunks` / `text-chunks`.
* Every field `parse_session_events` reads is unchanged in v3 (F8–F12).

Independently confirmed by review: no field, record type, or helper
(`event_millis`, `usage_from_step`, `collect_text_parts`, `user_image_blocks`,
`compaction_id`, `finalize_assistant`) behaves differently on a v3 log.

⇒ the fix is **filename resolution only**; `parse_session_events` is untouched.

### 1.7 Proposed fix **[R2 — rules 2 and 3 changed]**

**Single change point**: `read_session_log_text` in `src-tauri/src/parsers/deepseek.rs`.

Add `fn resolve_generation_log_path(session_dir: &Path) -> Option<PathBuf>`:

1. **Enumerate.** `read_dir(session_dir)`. For each **file** entry name: if it ends
   with `.zstd`, strip that suffix and tag encoding = `Zstd`, else encoding = `Raw`.
   Match the remainder against `^session(?:\.v([1-9][0-9]*))?\.jsonl$` — no capture ⇒
   generation 0, capture ⇒ parsed generation. Everything else is discarded, which
   naturally drops `session.lock`, `session.migration.<token><suffix>.tmp`,
   `session.v0.jsonl.zstd`, `session.V3.jsonl.zstd`, `session.v03.jsonl.zstd`.

2. **Pick the encoding first, the generation second. [R2]** Partition the candidates
   by encoding. If the `Zstd` set is non-empty, use it; otherwise use the `Raw` set.
   Within the chosen set, take the **highest generation**.

   *Why encoding-first rather than "highest generation wins across encodings":*
   upstream declares that a root belongs to exactly one encoding and that lookup
   *rejects* generations carrying the other suffix (F5). Comparing generations across
   encodings would therefore let a stale file from a foreign encoding outrank the live
   one. Preferring `Zstd` is the right disambiguation for codeg specifically, because
   `deepseek-acp` only ever writes zstd (F16) — a raw file sharing the root is either
   hand-placed or from a non-stock deployment. A mixed root is malformed by upstream's
   own rule, so any choice is a guess; codeg makes a **documented, deterministic** one
   rather than showing nothing. Emit `tracing::warn!` naming the ignored set when both
   are non-empty (precedent: `parsers/codex.rs:202`), so the misconfiguration is
   visible instead of silent.

   This preserves today's behaviour exactly for both well-formed shapes: a pure-zstd
   root and a pure-raw root.

3. **No fallback to a lower generation. [R2 — removed from revision 1]** If the
   selected candidate cannot be read, `read_session_log_text` returns `None`; if it
   decodes to zero bytes, the parse yields an empty session. It must **not** fall
   through to an older generation.

   *Why revision 1's fall-through was wrong:* a zero-byte or unreadable
   `session.v3.jsonl.zstd` beside a retained `session.jsonl.zstd` would have made
   codeg display the pre-migration history as if it were current — exactly the silent
   truncation of §1.5, and exactly what upstream forbids ("retained predecessors do
   not provide automatic fallback", F6). It was also unnecessary for the live-append
   race it was meant to cover: `decode_zstd_frames_prefix` already keeps every byte of
   every complete frame, so a torn tail is handled *within* the selected generation.
   An older generation is not a prefix of a newer one. Empty is the honest answer;
   stale-presented-as-current is not.

`read_session_log_text` then dispatches on the chosen path's suffix:
`decode_zstd_frames_prefix` for `.zstd`, `fs::read_to_string` otherwise.

Generation parsing uses plain string handling or the already-present `regex` crate
(F14) — no new dependency either way.

Also update the module doc-comment tree at `deepseek.rs:97-105`, which currently
documents only `session.jsonl.zstd  # or session.jsonl when compression=none`.

### 1.8 Proposed tests **[R2 — substantially rewritten]**

In the existing `#[cfg(test)]` module of `deepseek.rs`. `write_session(dir, bucket,
id, log, compressed)` gains a filename parameter and must keep writing **exactly one**
file per call, so no test accidentally gains a v0 companion.

Two disciplines apply to every test below, because review found revision 1's list
could pass for the wrong reasons:

* **Distinguishable payloads.** Whenever two generations coexist, their logs must
  carry different user text, and the assertion must name the expected text — a
  `conversations.len() == 1` assertion alone passes while reading the stale file.
* **Assert the resolver directly where the point is rejection.** `build_detail`
  returns an empty default on failure, so `get_conversation(...).expect(...)` alone
  can never fail. Rejection tests assert `resolve_generation_log_path(dir)` equals the
  expected path or `None`.

| # | Test | Pins |
|---|---|---|
| T1 | v3-only directory: `list_conversations().len() == 1` **and** the detail's turn count and user text match the v3 payload | the reported bug; red today |
| T2 | Retained predecessor: v0 log ("old") + v3 log ("new") in one directory → detail shows "new" | §1.5 silent truncation |
| T3 | Non-canonical names rejected: for each of `session.V3.jsonl.zstd`, `session.v0.jsonl.zstd`, `session.v03.jsonl.zstd`, `session.migration.abc.jsonl.zstd.tmp`, `session.lock`, assert `resolve_generation_log_path` does **not** select it; with a canonical `session.jsonl.zstd` also present it resolves to that one | revision 1's version passed vacuously because the canonical file existed |
| T4 **[R3]** | Raw encoding: a lone `session.v3.jsonl` ("raw-v3") → assert the **turn count and the user text**, not just that `get_conversation` returned | the raw branch; red today. A bare `.expect()` passes even if raw reading fails, because `build_detail` returns an empty default |
| T5 | Generation rule is not hardcoded: `session.v4.jsonl.zstd` ("four") beats `session.v3.jsonl.zstd` ("three") → detail shows "four" | future generations |
| T6 **[R2]** | **Torn tail on the selected generation, beside a stale valid v0**: v3 = complete frames + a half-written trailing frame ("new"), v0 = valid ("old") → detail shows the decoded v3 prefix, never "old" | the existing torn test calls `decode_zstd_frames_prefix` directly and cannot catch a wrong fallback |
| T7 **[R2]** | **Zero-byte selected generation beside a valid v0**: v3 is empty, v0 is valid ("old") → the session must **not** render "old" (it lists as absent / renders empty) | the removed fall-through stays removed |
| T8 **[R2]** | **Mixed-encoding root**: `session.v3.jsonl` (raw, "raw") + `session.jsonl.zstd` (zstd v0, "zstd") → resolves to the zstd set per §1.7 rule 2 | the documented disambiguation |
| T9 **[R3]** | **Undecodable selected generation beside a valid predecessor**: `session.v3.jsonl.zstd` holds bytes that are not a valid zstd frame, `session.jsonl.zstd` holds a valid v0 ("old") → the session must **not** render "old". Portable (no `chmod`), and it exercises the branch where the *read* succeeds but the *decode* yields nothing. Optionally add a `#[cfg(unix)]` twin using `chmod 000` for the true `fs::read` error path | T7 only covers a successful read of a zero-byte file; an implementation could fall back on `fs::read` errors alone and still pass T7 |
| T10 | The three existing tests stay green: `lists_and_loads_a_zstd_multi_frame_session` (canonical zstd v0 end to end), `torn_trailing_frame_keeps_the_decoded_prefix` (decoder-level), and — **load-bearing, do not weaken [R3]** — `skips_empty_plaintext_and_subagent_sessions` (`deepseek.rs:1736-1763`), which is the pin for **unversioned raw `session.jsonl`**: it writes a `plain` session with `compressed: false` and asserts `conversations.len() == 1`, and because `build_summary` drops any session with `content_events == 0`, that assertion fails outright if raw v0 resolution breaks | no regression on either v0 encoding |

**Mutation check** (repo discipline): for each of T1–T9, break the corresponding part
of the resolver, confirm that test goes red, restore.

Verification: `cd src-tauri && cargo test --features test-utils deepseek` and
`cargo clippy --all-targets --features test-utils -- -D warnings`.

---

## 2. Issue #733 — Missing English translations for DeepSeek Harness

### 2.1 Reporter's claim

On an English codeg UI the reasoning selector reads `高`, the file-permission selector
reads `可写工作区` with a dropdown of `只读`/`可写工作区`/`完全访问` plus Chinese
descriptions, and the permission buttons read `允许本次`/`拒绝`.

### 2.2 Verdict: REAL, but the root cause is **upstream**, not a missing codeg translation. Severity P1.

### 2.3 Verified facts

| # | Fact | How established |
|---|---|---|
| G1 | None of those strings exist anywhere in the codeg repo | full-repo grep |
| G2 | They come from `deepseek-acp@0.9.0`: `lib/config/options.js:17-20` (`关闭/低/高/最高`), `:34-49` (`只读/可写工作区/完全访问` + descriptions + a Windows "partial enforcement" sentence appended on `win32`), `:149,177,190` (`模型`/`推理档位`/`文件权限`); `lib/config/modes.js:25-29` (`常规`/`计划`); `lib/answerers/approval.js:17-18` (`允许本次`/`拒绝`) | read the tarball |
| G3 | The package has **no** locale/i18n mechanism; the only env vars it reads are `DEEPSEEK_API_KEY`, `DEEPSEEK_ACP_{PROVIDER,MODEL,SESSIONS_ROOT,LSP_SERVERS}` | grep of the tarball |
| G4 | Stable ids: options `model`/`reasoning`/`sandbox`; reasoning values `off`/`low`/`high`/`max`; sandbox values `read-only`/`workspace-write`/`danger-full-access`; modes `default`/`plan`; permission options `allow-once`/`reject-once` with ACP `kind` `allow_once`/`reject_once` | read the tarball |
| G5 | codeg renders them verbatim. Backend `map_session_modes` / `map_session_config_option` (`acp/connection.rs:2813,2860,2907,2919`) only `.clone()` | read the files |
| G6 | `AcpAgentSettings.codex.sandboxMode_{read-only,workspace-write,danger-full-access}` already exist in all 10 `src/i18n/messages/*.json` | grep |
| G7 | `AcpEvent::ConfigOptionRejected` carries `config_id`, `option_name`, `requested`, `actual` (`types.ts:2536-2541`) — but `requested`/`actual` are **display labels already resolved against the option's value list** (the type's own comment says so; produced by the `label()` closure at `acp/connection.rs:6979-6986`). The **value ids are not transmitted** | read both files |
| G8 | `PermissionDialog`'s two call sites (`conversation-shell.tsx:298`, `live-transcript-view.tsx:309`) both have `agentType` in scope | read the files |
| G9 | Model names/descriptions come from the user-editable `~/.dsh/settings.yaml`; the agent's system prompt is Chinese (`lib/launcher/boot.js:62-83`), which is why the model answers in Chinese | read the tarball |
| G10 **[R2]** | `snapshotLabels()` (`automations/agent-config-section.tsx:223,234,243`) **persists the agent's raw names** into the automation record as `mode_label` / `config_labels`, and `automations-page.tsx:1143-1152` renders those persisted labels. The stable ids are persisted too (`config.mode_id`, and `configEntries` is `[optionId, valueId]`), and `automation.agent_type` is available at that render (`:1095,1102`) | read the files |
| G11 **[R2]** | These components do **not** receive an agent type today: `AgentConfigSectionProps` (`agent-config-section.tsx:25`), `PanelPermissionCardProps` (`PanelPermissionCard.tsx:11`, fed by `SessionRow.tsx:92`), `SnapshotEditorProps` / `ModeRowProps` / `ConfigOptionRowProps` (`delegation-agent-defaults.tsx:280,339,392`). The values are available at each parent — the pet session type carries `agentType` (`lib/pet/types.ts:151,160`) and the delegation panel knows `selectedAgent` | read the files |
| G12 **[R2]** | `message-input.tsx` renders the config surface through **three** independent paths, not one: `ModelOptionPicker` for long model lists (`:1508`, which renders the option name in its tooltip and `aria-label`, `model-option-picker.tsx:63,72`), `InlineSessionConfigSelector` (`:1519`), and a separate `collapsedSettings` projection for the narrow composer (`:1541+`) that rebuilds `title`, `currentLabel`, and every option `name`/`description` | read the file |

### 2.4 Proposed fix **[R2 — scope widened]**

A codeg-side vocabulary override keyed on **agent type + stable id**, with verbatim
fallback.

1. New `src/lib/agent-label-vocabulary.ts` — pure functions plus lookup tables, active
   **only** for `AgentType.DeepSeek`. An id not in the table (or any other agent)
   passes through untouched, so an upstream-added tier does not disappear. Tables use
   the repo's `as const satisfies Record<…>` idiom so `t()` keeps literal key types.
2. New top-level i18n namespace (e.g. `AgentVocabulary.deepseek.*`) added to **all 10**
   `src/i18n/messages/*.json`. The three sandbox value names reuse the wording already
   at `AcpAgentSettings.codex.sandboxMode_*` (G6).
3. **Descriptions are written by codeg**, and the sandbox descriptions must keep an
   "enforcement may be partial on some platforms" qualifier — upstream appends a
   Windows partial-enforcement sentence at runtime (G2), and an unqualified
   translation would silently drop a safety caveat.
4. **Apply at every render site.** Revision 1 under-counted these; the full list:

   | Site | What needs the projection |
   |---|---|
   | `chat/message-input.tsx:1508` `ModelOptionPicker` **[R2]** | option name in tooltip + `aria-label` |
   | `chat/message-input.tsx:1519` `InlineSessionConfigSelector` | option name/description, value names/descriptions |
   | `chat/message-input.tsx:1530` `InlineModeSelector` | mode names/descriptions |
   | `chat/message-input.tsx` `collapsedSettings` projection **[R2]** | `title`, `currentLabel`, and every option `name`/`description` |
   | `chat/permission-dialog.tsx` | option names (new `agentType` prop; both owners have it, G8) |
   | `app/pet-panel/_components/PanelPermissionCard.tsx` **[R2]** | option names (new prop, fed from `SessionRow.tsx:92` / `session.agentType`) |
   | `settings/delegation-agent-defaults.tsx` **[R2]** | `SnapshotEditor` / `ModeRow` / `ConfigOptionRow` all need the agent type plumbed from `selectedAgent` |
   | `automations/agent-config-section.tsx` **[R2]** | `AgentConfigSectionProps` needs `agentType` from its automation / task-editor / task-settings owners |
   | `automations/automations-page.tsx:1143-1152` **[R2]** | see 5 below |
   | `contexts/acp-connections-context.tsx:3619-3634` | the rejection toast; see 6 below |

5. **Persisted automation labels [R2].** `snapshotLabels()` freezes the agent's raw
   name into the record (G10). Localising only at save time would (a) leave every
   automation saved before this change in Chinese forever and (b) freeze the label in
   whatever locale was active at save time. Fix at the **detail render**: resolve from
   `automation.agent_type` + the persisted stable ids (`config.mode_id`, and each
   `configEntries` key/value pair), falling back to the stored `mode_label` /
   `config_labels[k]`, then to the raw id. Keep persisting the agent's raw name — that
   is the existing durability intent ("keeps showing them even if the agent is later
   uninstalled").

6. **The rejection toast needs a small backend change [R2] ⇒ #733 is not
   frontend-only.** The event carries `config_id`, but `requested` / `actual` are
   display labels, not value ids (G7), so the frontend cannot key a lookup on them.
   Add the raw value ids to `AcpEvent::ConfigOptionRejected` at
   `acp/connection.rs:6987` (e.g. `requested_value` / `actual_value`, keeping the
   existing labels as fallback), mirror them in `types.ts`, and widen
   `reportConfigOptionVerdict`'s parameter type — it already receives `agentType` and
   the whole event. Reverse-mapping label → id on the frontend is explicitly rejected:
   the event is emitted *before* the `session_config_options` that carries the adopted
   value (see the comment at `acp-connections-context.tsx:4083-4085`), so the store
   may still hold the pre-update list.

7. **Tests.**
   * Pure lookup-table units: known id → localized; unknown id / unknown value →
     verbatim; non-DeepSeek agent → untouched.
   * Component-level: tooltip and `aria-label` text on `ModelOptionPicker`, the
     collapsed projection's `currentLabel`, pet-panel buttons, and the persisted
     automation-label render.
   * Make the new props **optional** so existing `PermissionDialog`,
     `PanelPermissionCard` and `AgentConfigSection` fixtures keep their current
     verbatim expectations without edits.
   * **[R3]** Update the existing `config_option_rejected` fixture at
     `src/contexts/acp-connections-context.test.tsx:2287-2295` with the two new value
     ids (and any exact serialization snapshot that carries the event payload).

### 2.5 Deliberately out of scope

* **Model names and descriptions** — user data from `~/.dsh/settings.yaml` (G9).
* `技能：` / `补充说明（可选）` in slash-command descriptions and
  `需要你回答 N 个问题` / `（只能选一项）` in elicitation — no stable key; only
  matchable by Chinese literal, which is too brittle.
* The terminal-auth method name `在终端里登录` — codeg's frontend never renders ACP
  auth-method names.
* The agent's Chinese **system prompt** (G9) — not a UI string; upstream issue.

### 2.6 Rejected alternative

Rewriting the labels in Rust at `map_session_modes` / `map_session_config_option`
would be one choke point covering every surface, but codeg's 10-locale text lives in
the frontend JSON; doing it in Rust means a second translation store (only
`commands/windows.rs` window titles do that today). Note that item 6 above still
requires a *data* change in Rust — that is transmitting ids, not translating text.

---

## 3. Sequencing and follow-ups

1. #734 first — data visibility loss, one function plus tests, and codeg's own pin is
   the affected version (F13).
2. #733 second — larger than revision 1 assumed: 10 render sites, 4 new props, one
   small backend event change, 10 locale files.
3. Two upstream issues: (a) the ACP vocabulary has no i18n — request an English
   default or a locale switch; (b) the session-log generation rename is a silent
   breaking change for third-party readers and belongs in the release notes.

---

## 4. What revision 2 changed

**#734**
* Removed the "fall back to the next-highest generation" step — it re-introduced the
  §1.5 stale-history failure and was unnecessary given `decode_zstd_frames_prefix`.
* Replaced "highest generation across encodings" with "pick the encoding set first
  (zstd wins), then the highest generation within it", with the rationale and a
  `tracing::warn!` for the malformed mixed-encoding case.
* Rewrote the test list: added T6 (torn tail beside stale v0), T7 (zero-byte beside
  valid v0), T8 (mixed encoding); made T3 assert the resolver directly instead of
  passing vacuously; required distinguishable payloads and content assertions
  throughout; added the mutation check.
* Recorded F15 (single change point, independently confirmed) and F16 (agent always
  writes zstd).

**#733**
* Added three missed surfaces: `ModelOptionPicker`, the `collapsedSettings`
  projection, and the persisted automation labels rendered on the automations detail
  page.
* Corrected "only `PermissionDialog` needs the agent type" — three more components
  need it plumbed (G11).
* Added the backend change needed for the rejection toast: the event transmits labels,
  not value ids (G7). #733 is therefore not frontend-only.
* Added component-level tests and the optional-prop rule for existing fixtures.

## 5. Revision 3 — second review round **[R3]**

The revised §1.7 rules and the whole of §2 were **approved** on re-review. Three
test-plan gaps remained and are now closed:

* T4 asserts content, not just that the call returned.
* New T9: an *undecodable* selected generation beside a valid predecessor. T7 only
  covered a successful read of a zero-byte file, so an implementation that fell back
  on `fs::read` errors alone would have passed.
* T10 now names `skips_empty_plaintext_and_subagent_sessions` explicitly.

**One finding pushed back with evidence.** The reviewer reported "legacy raw
generation 0 is untested — an implementation that mishandles only unversioned raw
logs can pass T1–T9." It is in fact covered: `skips_empty_plaintext_and_subagent_sessions`
(`deepseek.rs:1741-1763`) writes a `plain` session with `compressed: false` (i.e.
`session.jsonl`) and asserts `conversations.len() == 1`; since `build_summary` returns
`None` for any session with `content_events == 0`, breaking raw v0 resolution makes
that assertion fail. Rather than adding a duplicate test, T10 now names it as
load-bearing so an implementer does not weaken it.

### Verification status

No code has been written, so there is no check suite to run — the only changed file is
this document. The commands the implementer must run are specified in §1.8
(`cargo test --features test-utils deepseek`, `cargo clippy --all-targets --features
test-utils -- -D warnings`) and, for #733, `pnpm test` plus `pnpm eslint src`.

---

## 6. What shipped

### #734 — `168117f6`

`src-tauri/src/parsers/deepseek.rs` only. `parse_generation_log_filename` +
`resolve_generation_log_path` replace the two hardcoded names; `read_session_log_text`
dispatches on the resolved encoding. `parse_session_events` is byte-for-byte unchanged,
as the plan predicted.

Ten new tests, all of §1.8 plus the `#[cfg(unix)]` permission twin. **Mutation check
green**: five mutations, one per rule, each caught by at least one test —

| Mutation | Caught by |
|---|---|
| accept `.v0` / leading zeros as canonical | `only_canonical_generation_names_are_selected`, `non_canonical_neighbours_do_not_displace_the_canonical_log` |
| compare generations across encodings | `a_mixed_encoding_root_reads_the_compressed_set` |
| fall back to a lower generation | all three `does_not_fall_back_to_the_predecessor` tests |
| take the lowest generation | 6 tests |
| drop the `is_file` guard | `non_canonical_neighbours_do_not_displace_the_canonical_log` |

### #733 — `24632320`

`src/lib/agent-label-vocabulary.ts` (pure tables + helpers) and
`src/hooks/use-agent-vocabulary.ts` (the render-time hook), 25 keys added to all 10
locale catalogues, plus the wiring from §2.4.

Two places came out simpler than planned, one the same, one as feared:

* **The composer needed one change, not three.** Localising `availableModes` /
  `availableConfigOptions` at their source memos covers the model picker, the inline
  dropdowns and the collapsed projection at once, because all three read those arrays.
* **`delegation-agent-defaults` and `agent-config-section` needed no row changes** —
  localising the snapshot where the rows are built covers `ModeRow` /
  `ConfigOptionRow` and the "agent default" caption they derive.
* **The persisted automation labels** went exactly as §2.4 item 5 describes, with a
  regression test that a frozen Chinese `label_snapshot` renders in English while the
  user's own model name still stands.
* **The rejection toast did need Rust.** `AcpEvent::ConfigOptionRejected` now carries
  `requested_value` / `actual_value`; the TS mirror makes them optional so an older
  server still type-checks.

### Verification

| Check | Result |
|---|---|
| `cargo test --features test-utils --lib` | 3678 passed, 0 failed |
| `cargo clippy --all-targets --features test-utils -- -D warnings` | clean |
| `cargo test --no-default-features --bin codeg-server --lib` | 3639 passed, 0 failed |
| `cargo clippy --no-default-features --bin codeg-server --lib -- -D warnings` | clean |
| `cargo clippy --no-default-features --bin codeg-mcp -- -D warnings` | clean |
| `npx vitest run` | 437 files, 6351 tests passed |
| `npx eslint src/` | clean (1 pre-existing warning in `status-bar-mcp.tsx`) |
| `npx tsc --noEmit` | ~~clean~~ — **reported wrongly here; see section 7** |
| `pnpm build` | static export succeeds — also the real ICU compile of all 10 catalogues |

One flake seen and dismissed: `chat_channel::webhook::tests::post_one_reports_non_2xx`
failed once in the server-mode run and passed on re-run both alone and in a full
re-run. It spins a local TCP listener and asserts a 500 propagates; it touches nothing
in either change, and the desktop run of the same test was green.

---

## 7. Third review round — against the CODE **[R4]**

Rounds 1–2 reviewed the plan. This round reviewed the shipped diff, in four fenced
slices (Rust log resolution; the ACP event + its consumer; the vocabulary module +
hook; call-site coverage + translation semantics). Two slices approved outright;
the other two raised seven Important findings, in three classes.

### 7.1 Accepted and fixed

**The generation integer was too narrow.** `parse_generation_log_filename` parsed
`u32`, but upstream accepts `1..=Number.MAX_SAFE_INTEGER`. That is not a cosmetic
gap: a generation the integer cannot hold stops *being* a generation, so a retained
predecessor beside it would win — the exact silent-truncation failure §1.5 exists to
prevent. Now `u64` with upstream's ceiling checked explicitly.

**A listing that breaks part way through could crown a predecessor.**
`entries.filter_map(|e| e.ok())` discarded a mid-stream `readdir` failure — real on
NFS/FUSE — and then selected from whatever had already arrived. Same truncation
class. An `Err` is now fatal to the selection: nothing is the honest answer, exactly
as for an unreadable newest generation. The loop moved into `select_generation_log`
so a test can inject a listing that fails, which no temporary directory can be made
to do.

**Four call sites named the wrong agent.** `AgentConfigSection` and `SnapshotEditor`
were handed the *currently selected* agent while still rendering the *previous*
agent's probe snapshot — both loaders debounce the re-probe (250 ms /
`TAB_SWITCH_DEBOUNCE_MS`) and only clear the snapshot inside the debounced load.
The reviewer framed this as "switching away from DeepSeek flashes Chinese", which is
merely the old behaviour; the direction this change actually creates is switching
*toward* DeepSeek, where the DeepSeek tables get applied to another agent's snapshot.
That is live, not hypothetical: `connection.rs:3007` normalises every agent's model
option to id `model`, and codex's sandbox values are literally `read-only` /
`workspace-write` / `danger-full-access`. Fixed by storing the snapshot and its
producing agent in one state value in both loaders, so they cannot render out of
step; `useAgentOptions` exposes `snapshotAgentType`.

### 7.2 Disputed, and the dispute was accepted

Resolution now needs permission to *enumerate* the session directory, where the old
code only opened a fixed path — so a mode-`0111` directory would regress. The
mechanism is real (verified: `chmod 0111` denies `ls` while `cat dir/file` still
works), but enumeration is inherent to "select the highest generation" over an
unbounded generation space, and the only mitigation — probe generation 0 when
`read_dir` fails — reintroduces the silent-truncation path precisely when we know
least about what exists. `deepseek-acp` creates these directories with the default
umask. Recorded as non-blocking.

### 7.3 Correction to section 6

**`npx tsc --noEmit` was not clean** when section 6 was written. It reported three
errors, all in test code added by this change: a `created_at` field that is not on
`PendingPermission` (twice), and a spread of the nullable `Automation.config`.
Vitest does not type-check, so the green suite hid them. Fixed; `tsc` now exits 0.

### 7.4 Left alone, deliberately

`delegation-agent-defaults.tsx`'s cache-hit path does not bump `reqIdRef`, so a slow
probe for a previous agent can still overwrite a newly-cached one — the sibling
`useAgentOptions` fixes this explicitly ("bump FIRST so a cache hit also invalidates
any still-in-flight probe"). Pre-existing, orthogonal to this change, and the
snapshot/agent pairing above makes its symptom *less* wrong rather than more.

### 7.5 Verification after the fixes

| Check | Result |
|---|---|
| `cargo test --features test-utils --lib` | 3679 passed, 0 failed, 1 ignored |
| `cargo clippy --all-targets --features test-utils -- -D warnings` | clean |
| `cargo test --no-default-features --bin codeg-server --lib` | 3640 passed, 0 failed, 1 ignored (no flake this run) |
| `cargo clippy --no-default-features --bin codeg-server --lib -- -D warnings` | clean |
| `cargo clippy --no-default-features --bin codeg-mcp -- -D warnings` | clean |
| `npx vitest run` | 438 files, 6352 tests passed |
| `npx eslint src/` | clean (same 1 pre-existing warning) |
| `npx tsc --noEmit` | clean (exit 0) |
| `pnpm build` | static export succeeds |

Mutation check on the new behaviour, 4 mutations, all caught red: `u64`→`u32`;
partial-listing `return None`→`continue`; drop the `MAX_SAFE_INTEGER` guard;
`snapshotAgentType: loaded?.agent` → `agentType`.
