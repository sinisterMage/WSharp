# Criterion 4 gate recovery

The existing implementation and CI job are preserved in PR #45, stacked on
PR #36. Recovery tightens two fail-closed paths: missing authentication exits 2,
and an unlabelled issue is fixed only when GitHub reports completed explicitly.
A null or unknown closure reason is not proof of a fix.

Regression command (Linux, Bash):

~~~sh
bash scripts/tests/followups-check.test.sh
~~~

With the new tests and the original gate at dce0fd5, this exits 1: both
"unknown closure reason is not fixed" and "missing token fails closed" report
exit 0 instead of the required exit 2. With the fix, all 19 offline checks pass.
These tests also cover open issues, missing sections, newly labelled subjects,
and failed fetches. They never use live GitHub access or skip a failed gate.

Run the live acceptance check separately with a valid GITHUB_TOKEN or GH_TOKEN:

~~~sh
bash scripts/followups-check.sh
bash scripts/gate-table-check.sh
~~~

Limitations: the API adapter refuses a full 100-item page (exit 2); pagination
must be implemented before the roster reaches that size. Documentation checks
assert section references and field markers, not the truth or completeness of
prose. Human release review remains necessary. The script checks the supplied
checkout; release evidence must additionally establish that LIMITATIONS.md is
on main. Tests inject FOLLOWUPS_FETCH; release runs must leave it unset.

## Live verification on 2026-09-27

Against main `6c0cce3585398aa3d1f69c9d1c05c0c4c3fef22d`, the authenticated
`bash scripts/followups-check.sh` exits 1: the original six are documented,
but newly labelled #47 is open. This is a real criterion failure, not a fetch
failure. PR #58 carries its documented disposition and closure; Johnny owns
that existing review/merge workflow. Rerun this command after it lands, using
main documentation, before claiming criterion 4 met.

The state table has twelve written checks and two unwritten checks. Its
checker still validates eleven of fourteen rows: writing this gate changes
a verdict, not the number of rows whose paths can be checked.

## Passing live acceptance on 2026-09-27

PR #58 merged as main `4e9249a6850f9721db13dbd61c63c57250eced11`
after the approved head and current protected-branch check requirements were
verified. GitHub automatically closed #47; its `v1.0-limitation` label remains.
The merged `LIMITATIONS.md` includes its documented disposition.

After integrating that main commit into PR #45, these commands passed:

~~~sh
bash scripts/tests/followups-check.test.sh # exit 0; 19 checks
bash scripts/tests/gate-table-check.test.sh # exit 0; 18 checks
env -u FOLLOWUPS_FETCH bash scripts/followups-check.sh # exit 0
bash scripts/gate-table-check.sh # exit 0; 11 of 14 rows checked
~~~

The live run used managed GH_TOKEN and the unchanged main documentation. It
reported `CRITERION 4 MET: 7 of 7 subjects resolved or documented.` This
supersedes the historical open-#47 result above. It is criterion 4 evidence,
not clean-install evidence. Final PR-head CI and Johnny's review remain
separate requirements. No release or publishing action was performed.

## Runtime CI repair integration on 2026-09-27

The Debian check on head `4f118046` failed in
`worker::tests::the_shared_poll_flag_counts_workers`; the other four platform
arms completed successfully. Dex traced the failure to pause publication
ordering and independently verified Mira's PR #63 at `7bb4a37` on Debian 13,
glibc 2.41, rustc 1.95.0. PR #45 now includes that branch's original regression
and repair commits. See `docs/runtime-poll-protocol.md` for exact failing-before
and passing-after commands and the remaining cross-worker limitation.

After integration, both checker selftests passed (19 and 18 checks),
`env -u FOLLOWUPS_FETCH bash scripts/followups-check.sh` exited 0 with all seven
subjects documented, and `bash scripts/gate-table-check.sh` exited 0.
`git diff --exit-code origin/main -- LIMITATIONS.md` exited 0 against main
`4e9249a6850f9721db13dbd61c63c57250eced11`; `git diff --check` also exited 0.
These are focused gate results. Final-head platform CI must complete before
renewed review; prior PR #63 platform results do not certify this integrated
head. No clean-install or release claim is made.
