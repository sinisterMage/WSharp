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
