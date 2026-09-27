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
