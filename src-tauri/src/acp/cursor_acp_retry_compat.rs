//! Version-gated compatibility patch for Cursor ACP sessions that omit
//! `enableAgentRetries` from `agentClient.run` options (see `agent-session.ts`).
//!
//! Cursor's ACP entry point builds its run options WITHOUT the flag, while the
//! TUI/headless path sets it from `retry-helpers.ts`'s `w5(action.case)`. The
//! run loop reads it as `null!==(g=_.enableAgentRetries)&&void 0!==g&&g` and
//! gates EVERY retry branch on `enableAgentRetries || endlessRetries`, so an
//! ACP session gets `undefined` -> `false` -> zero retries: the first transport
//! blip, stall or retryable server error ends the turn outright, where the TUI
//! would retry it (10 transport attempts / 3 server attempts). There is no
//! CLI flag, config key or env var that reaches the flag from outside, which
//! is why this is a bundle patch rather than a launch-argument change.
//!
//! This is NOT a regression in one release — `2026.08.11-e8db854` omits the
//! flag too — but the fix stays pinned to versions whose bytes were actually
//! inspected: the replacement is an exact splice into minified code whose
//! identifiers (`I`, `S`, `P` below) are regenerated on every Cursor build, so
//! applying it blind to an unknown bundle is how you get a `ReferenceError` at
//! run time instead of a retry. `pinned_cursor_version_is_triaged` keeps that
//! list honest when the registry pin moves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::acp::error::AcpError;

const CURSOR_AGENT_ID: &str = "cursor";

/// Cursor agent-cli versions whose bundle was inspected and found to omit
/// `enableAgentRetries` from the ACP `agentClient.run` options.
///
/// `2026.09.10-fd3934a` re-inspected: still affected, and the splice below
/// still applies verbatim. Its ACP chunk (`dist-package/7214.index.js`) has
/// zero `enableAgentRetries` occurrences, [`VULNERABLE_RUN_OPTIONS`] matches it
/// exactly once — so `S`/`P` are unchanged — and the run request one statement
/// above the call is still `I = new u.ConversationAction({action: {case:
/// "userMessageAction", …}})`, which is what makes `I.action.case` in
/// [`PATCHED_RUN_OPTIONS`] both in scope and correct.
const AFFECTED_VERSIONS: &[&str] = &["2026.09.02-c22c1a3", "2026.09.10-fd3934a"];

/// Cursor agent-cli versions whose bundle was inspected and found to already
/// pass `enableAgentRetries` on the ACP path (upstream fixed it, or the code
/// moved). Kept beside [`AFFECTED_VERSIONS`] so the triage test can tell
/// "checked, nothing to do" apart from "nobody has looked at this pin yet".
///
/// Read only by `pinned_cursor_version_is_triaged`, but it belongs next to
/// [`AFFECTED_VERSIONS`] — that pair is the triage record a maintainer bumping
/// the Cursor pin has to update.
#[allow(dead_code)]
const UNAFFECTED_VERSIONS: &[&str] = &[];

const AGENT_SESSION_MODULE: &str = "\"./src/acp/agent-session.ts\"";

/// Vulnerable minified runOptions tail in affected bundles (no `enableAgentRetries`).
const VULNERABLE_RUN_OPTIONS: &str =
    ")),{onConnectionStateChange:e=>{\"reconnecting\"===e.state?(0,S.debugLog)(\"Connection state: reconnecting\"):\"connected\"===e.state&&(0,S.debugLog)(\"Connection state: connected\")},onErrorNotRetried:e=>{(0,P.Z)({configProvider:this.sharedServices.configProvider,info:e})}})";

/// Patched runOptions: inline equivalent of `w5(I.action.case)`, whose body in
/// the affected bundle is exactly
/// `function u(e){return"shellCommandAction"!==e&&"backgroundTaskCompletionAction"!==e&&"goalContinuationAction"!==e}`.
///
/// `agent-session.ts` has a single `agentClient.run` call and it always builds
/// a `userMessageAction`, so today the expression is always `true`; it is
/// written as the policy rather than a bare `true` so that a bundle which
/// later routes another action through the same call site still gets
/// upstream's answer instead of ours.
const PATCHED_RUN_OPTIONS: &str =
    ")),{enableAgentRetries:\"shellCommandAction\"!==I.action.case&&\"backgroundTaskCompletionAction\"!==I.action.case&&\"goalContinuationAction\"!==I.action.case,onConnectionStateChange:e=>{\"reconnecting\"===e.state?(0,S.debugLog)(\"Connection state: reconnecting\"):\"connected\"===e.state&&(0,S.debugLog)(\"Connection state: connected\")},onErrorNotRetried:e=>{(0,P.Z)({configProvider:this.sharedServices.configProvider,info:e})}})";

/// Marker written by this patch or upstream fixes.
const ENABLE_AGENT_RETRIES_MARKER: &str = "enableAgentRetries:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatPatchStatus {
    NotApplicable,
    AlreadyFixed,
    Applied,
    PatternMismatch,
}

impl CompatPatchStatus {
    pub fn log_label(self) -> &'static str {
        match self {
            Self::NotApplicable => "NOT_APPLICABLE",
            Self::AlreadyFixed => "ALREADY_FIXED",
            Self::Applied => "APPLIED",
            Self::PatternMismatch => "PATTERN_MISMATCH",
        }
    }
}

/// Apply the compatibility patch when `platform_dir` holds a managed Cursor
/// install for a known-affected version.
///
/// Idempotent, and best-effort by design: every way of not recognising the
/// bundle collapses to `PatternMismatch`, which leaves Cursor's bytes exactly
/// as shipped. Callers deliberately ignore the status — an unpatched agent is
/// the status quo (fewer retries), while refusing to launch over a failed
/// patch would turn a reliability nicety into an outage.
///
/// Re-reads the bundle on every call; the process-wide entry points
/// ([`maybe_apply_for_agent`] / [`apply_after_install_for_agent`]) are the ones
/// that bound that cost.
pub fn maybe_apply(platform_dir: &Path, version: &str) -> CompatPatchStatus {
    let normalized = normalize_version_label(version);
    if !AFFECTED_VERSIONS.iter().any(|v| *v == normalized) {
        return CompatPatchStatus::NotApplicable;
    }

    let dist_package = platform_dir.join("dist-package");
    if !dist_package.is_dir() {
        log_status(CompatPatchStatus::PatternMismatch, &normalized, None);
        return CompatPatchStatus::PatternMismatch;
    }

    let bundle_path = match find_agent_session_bundle(&dist_package) {
        Some(path) => path,
        None => {
            log_status(CompatPatchStatus::PatternMismatch, &normalized, None);
            return CompatPatchStatus::PatternMismatch;
        }
    };

    let content = match std::fs::read_to_string(&bundle_path) {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!(
                "Cursor ACP retry compatibility patch: read failed (version={}, bundle={}, err={})",
                normalized,
                bundle_path.display(),
                err
            );
            log_status(
                CompatPatchStatus::PatternMismatch,
                &normalized,
                Some(&bundle_path),
            );
            return CompatPatchStatus::PatternMismatch;
        }
    };

    if !content.contains(AGENT_SESSION_MODULE) {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    if content.contains(ENABLE_AGENT_RETRIES_MARKER) {
        log_status(
            CompatPatchStatus::AlreadyFixed,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::AlreadyFixed;
    }

    if !content.contains(VULNERABLE_RUN_OPTIONS) {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    let patched = content.replace(VULNERABLE_RUN_OPTIONS, PATCHED_RUN_OPTIONS);
    if patched == content {
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    if let Err(err) = write_atomically(&bundle_path, &patched) {
        tracing::warn!(
            "Cursor ACP retry compatibility patch: write failed (version={}, bundle={}, err={})",
            normalized,
            bundle_path.display(),
            err
        );
        log_status(
            CompatPatchStatus::PatternMismatch,
            &normalized,
            Some(&bundle_path),
        );
        return CompatPatchStatus::PatternMismatch;
    }

    log_status(CompatPatchStatus::Applied, &normalized, Some(&bundle_path));
    CompatPatchStatus::Applied
}

/// How many times the same install may answer `PatternMismatch` before that is
/// taken as settled.
///
/// `Applied` / `AlreadyFixed` are stable by construction and memoized on the
/// first pass, but `PatternMismatch` is also where every transient failure
/// lands — a read that lost a race with an antivirus scan, a `rename` the
/// filesystem refused. Memoizing the first of those would suppress the patch
/// for the rest of the process; retrying it forever would put the ~9 MB scan
/// back on a hot path. A few attempts is both.
const MAX_MISMATCH_ATTEMPTS: u32 = 3;

/// Outcomes already reached for a managed install in this process, with the
/// number of attempts behind each.
///
/// Resolving the bundle means reading every `*.index.js` chunk in
/// `dist-package` until the ACP one turns up (~9 MB across ~70 files for
/// Cursor), and the cache-hit hook runs on every connect, preflight and
/// diagnostics call — so without this the same megabytes are re-read, with
/// blocking I/O on an async worker, for the lifetime of the app. Locking it
/// also serializes patch attempts, so two callers never write the same bundle
/// at once.
fn attempted() -> &'static Mutex<HashMap<PathBuf, (CompatPatchStatus, u32)>> {
    static ATTEMPTED: OnceLock<Mutex<HashMap<PathBuf, (CompatPatchStatus, u32)>>> = OnceLock::new();
    ATTEMPTED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Patch a managed install that is being REUSED from the cache. At most one
/// bundle scan per install per process — see [`attempted`].
pub fn maybe_apply_for_agent(
    agent_id: &str,
    platform_dir: &Path,
    version: &str,
) -> CompatPatchStatus {
    apply_for_agent(agent_id, platform_dir, version, false)
}

/// Patch a managed install that was just downloaded and extracted.
///
/// Bypasses (and refreshes) the memo: a fresh extraction replaces the very
/// bytes an earlier outcome described, so a re-install after `clear_agent_cache`
/// must not inherit the previous install's "already handled".
pub fn apply_after_install_for_agent(
    agent_id: &str,
    platform_dir: &Path,
    version: &str,
) -> CompatPatchStatus {
    apply_for_agent(agent_id, platform_dir, version, true)
}

fn apply_for_agent(
    agent_id: &str,
    platform_dir: &Path,
    version: &str,
    force: bool,
) -> CompatPatchStatus {
    if agent_id != CURSOR_AGENT_ID {
        return CompatPatchStatus::NotApplicable;
    }
    // Held across the patch so a concurrent caller waits for the outcome
    // rather than racing it onto the same file.
    let Ok(mut attempted) = attempted().lock() else {
        // A poisoned lock means a previous attempt panicked mid-patch; do not
        // touch the bundle again on the strength of that.
        return CompatPatchStatus::PatternMismatch;
    };
    let previous = attempted.get(platform_dir).copied();
    if !force {
        if let Some((status, attempts)) = previous {
            if status != CompatPatchStatus::PatternMismatch || attempts >= MAX_MISMATCH_ATTEMPTS {
                return status;
            }
        }
    }
    let status = maybe_apply(platform_dir, version);
    if status != CompatPatchStatus::NotApplicable {
        // A fresh install starts its own attempt budget: the bytes the earlier
        // mismatches were counted against are gone.
        let attempts = if force {
            1
        } else {
            previous.map_or(1, |(_, attempts)| attempts + 1)
        };
        attempted.insert(platform_dir.to_path_buf(), (status, attempts));
    }
    status
}

fn find_agent_session_bundle(dist_package: &Path) -> Option<PathBuf> {
    let mut chunks: Vec<PathBuf> = std::fs::read_dir(dist_package)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".index.js"))
                && path.is_file()
        })
        .collect();
    // `read_dir` yields in filesystem order, so sort: which chunk we inspect
    // first — and therefore what a malformed sibling can perturb — should not
    // depend on the machine.
    chunks.sort();

    for path in chunks {
        // A chunk we cannot read, or that is not UTF-8, is simply not ours.
        // Returning here instead would abandon the search for the ACP chunk
        // because of an unrelated file and leave the bundle unpatched.
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.contains(AGENT_SESSION_MODULE) {
            return Some(path);
        }
    }
    None
}

fn write_atomically(path: &Path, content: &str) -> Result<(), AcpError> {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| AcpError::DownloadFailed("bundle path has no parent".into()))?;
    // Per-writer name: a fixed one lets a second writer truncate the staging
    // file another is still filling, and the first `rename` then publishes a
    // half-written bundle over Cursor's chunk.
    let tmp = parent.join(format!(
        ".codeg-acp-retry-patch-{}-{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = std::fs::write(&tmp, content.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(AcpError::DownloadFailed(format!(
            "write temp bundle patch: {e}"
        )));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Never leave staging bytes inside the agent's own package dir.
        let _ = std::fs::remove_file(&tmp);
        return Err(AcpError::DownloadFailed(format!(
            "commit bundle patch: {e}"
        )));
    }
    Ok(())
}

fn normalize_version_label(version: &str) -> String {
    let trimmed = version.trim();
    if let Some(stripped) = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
    {
        stripped.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn log_status(status: CompatPatchStatus, version: &str, bundle: Option<&Path>) {
    match bundle {
        Some(path) => tracing::info!(
            "Cursor ACP retry compatibility patch: {} (version={}, bundle={})",
            status.log_label(),
            version,
            path.display()
        ),
        None => tracing::info!(
            "Cursor ACP retry compatibility patch: {} (version={})",
            status.log_label(),
            version
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vulnerable_fixture() -> String {
        format!(
            "exports.modules={{{}(e,t,o){{placeholder {} end}}}}",
            AGENT_SESSION_MODULE, VULNERABLE_RUN_OPTIONS
        )
    }

    fn write_bundle(dir: &Path, content: &str) -> PathBuf {
        let dist = dir.join("dist-package");
        std::fs::create_dir_all(&dist).unwrap();
        let path = dist.join("2471.index.js");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn unknown_version_is_not_applicable() {
        let tmp = tempfile::tempdir().unwrap();
        let status = maybe_apply(tmp.path(), "2026.08.11-e8db854");
        assert_eq!(status, CompatPatchStatus::NotApplicable);
    }

    #[test]
    fn affected_version_without_bundle_is_pattern_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let status = maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]);
        assert_eq!(status, CompatPatchStatus::PatternMismatch);
    }

    #[test]
    fn affected_vulnerable_bundle_is_patched_once() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        let status = maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]);
        assert_eq!(status, CompatPatchStatus::Applied);
        let bundle = find_agent_session_bundle(&tmp.path().join("dist-package")).unwrap();
        let content = std::fs::read_to_string(bundle).unwrap();
        assert!(content.contains(PATCHED_RUN_OPTIONS));
        assert!(content.contains("enableAgentRetries:\"shellCommandAction\"!==I.action.case"));
    }

    #[test]
    fn second_application_is_already_fixed() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::AlreadyFixed
        );
    }

    #[test]
    fn upstream_fixed_bundle_is_already_fixed() {
        let tmp = tempfile::tempdir().unwrap();
        let content = vulnerable_fixture().replace(
            VULNERABLE_RUN_OPTIONS,
            ")),{enableAgentRetries:(0,w5.w5)(I.action.case),onConnectionStateChange:e=>{}",
        );
        write_bundle(tmp.path(), &content);
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::AlreadyFixed
        );
    }

    #[test]
    fn unexpected_bundle_structure_is_pattern_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(
            tmp.path(),
            "exports.modules={{\"./src/acp/agent-session.ts\"(){broken",
        );
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::PatternMismatch
        );
    }

    #[test]
    fn patch_preserves_w5_action_policy() {
        assert!(PATCHED_RUN_OPTIONS.contains("\"shellCommandAction\"!==I.action.case"));
        assert!(PATCHED_RUN_OPTIONS.contains("\"backgroundTaskCompletionAction\"!==I.action.case"));
        assert!(PATCHED_RUN_OPTIONS.contains("\"goalContinuationAction\"!==I.action.case"));
    }

    // Cursor's `dist-package` holds ~70 webpack chunks. A chunk that is not
    // readable as UTF-8 (or that we simply cannot open) is not ours, and must
    // not abandon the scan for the one that is — otherwise the bundle is left
    // unpatched for a reason that has nothing to do with it.
    #[test]
    fn unreadable_sibling_chunk_does_not_abort_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        // Sorts before the `2471.index.js` the fixture writes, so the scan
        // reaches it first on every filesystem.
        std::fs::write(
            tmp.path().join("dist-package").join("0001.index.js"),
            [0xff_u8, 0xfe, 0xfd],
        )
        .unwrap();
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );
    }

    #[test]
    fn non_cursor_agent_id_is_not_applicable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            maybe_apply_for_agent("opencode", tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::NotApplicable
        );
    }

    // The cache-hit hook runs on every connect / preflight / diagnostics call,
    // so the ~9 MB chunk scan behind it must happen at most once per install.
    // Proven by removing the tree the answer came from: a second call that
    // still answers `Applied` cannot have gone back to disk.
    #[test]
    fn cache_hit_hook_resolves_each_install_once() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );

        std::fs::remove_dir_all(tmp.path().join("dist-package")).unwrap();
        assert_eq!(
            maybe_apply(tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::PatternMismatch,
            "the uncached path must see the tree is gone"
        );
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied,
            "the cache-hit hook must answer from the memo, not re-read the tree"
        );
    }

    // `clear_agent_cache` + re-download reuses the same platform dir, so the
    // post-install hook has to look at the NEW bytes. Inheriting the previous
    // install's outcome would leave a freshly extracted bundle unpatched.
    #[test]
    fn post_install_hook_ignores_the_previous_installs_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );

        // Re-extraction puts an unpatched bundle back under the same path.
        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            apply_after_install_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied
        );
        let bundle = find_agent_session_bundle(&tmp.path().join("dist-package")).unwrap();
        assert!(std::fs::read_to_string(bundle)
            .unwrap()
            .contains(PATCHED_RUN_OPTIONS));
    }

    // Every transient failure lands on `PatternMismatch` too, so memoizing the
    // first one would let a momentary hiccup — a locked file, a refused rename
    // — leave the agent unpatched for the rest of the session.
    #[test]
    fn a_transient_mismatch_does_not_settle_the_install() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::PatternMismatch
        );

        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied,
            "a later call must still be allowed to look"
        );
    }

    // ...but an install that keeps mismatching is an install whose bytes we do
    // not recognise, and re-reading its chunks on every connect / preflight /
    // diagnostics call would be pure waste.
    #[test]
    fn a_persistent_mismatch_stops_rescanning() {
        let tmp = tempfile::tempdir().unwrap();
        for _ in 0..MAX_MISMATCH_ATTEMPTS {
            assert_eq!(
                maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
                CompatPatchStatus::PatternMismatch
            );
        }

        write_bundle(tmp.path(), &vulnerable_fixture());
        assert_eq!(
            maybe_apply_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::PatternMismatch,
            "the budget is spent; the cache-hit hook stops looking"
        );
        assert_eq!(
            apply_after_install_for_agent(CURSOR_AGENT_ID, tmp.path(), AFFECTED_VERSIONS[0]),
            CompatPatchStatus::Applied,
            "a re-install still gets a fresh look"
        );
    }

    // The patch is a byte-exact splice, so it silently stops doing anything
    // the moment the registry pin moves. Fail here instead: whoever bumps
    // Cursor has to open the new bundle and put the version in one of the two
    // lists — `AFFECTED_VERSIONS` (re-derive the splice against the new
    // minified identifiers) or `UNAFFECTED_VERSIONS` (upstream fixed it).
    #[test]
    fn pinned_cursor_version_is_triaged() {
        let pinned = crate::acp::registry::get_agent_meta(crate::models::agent::AgentType::Cursor)
            .registry_version()
            .expect("Cursor pins a registry version");
        let pinned = normalize_version_label(pinned);
        assert!(
            AFFECTED_VERSIONS.contains(&pinned.as_str())
                || UNAFFECTED_VERSIONS.contains(&pinned.as_str()),
            "cursor-agent {pinned} has not been checked for the ACP \
             `enableAgentRetries` omission. Inspect the ACP chunk \
             ({AGENT_SESSION_MODULE}) in its `dist-package`: if the \
             `agentClient.run` options still lack `enableAgentRetries`, update \
             VULNERABLE_RUN_OPTIONS / PATCHED_RUN_OPTIONS for that build's \
             minified identifiers and add it to AFFECTED_VERSIONS; otherwise \
             add it to UNAFFECTED_VERSIONS."
        );
    }
}
