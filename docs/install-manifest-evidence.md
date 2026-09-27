# Criterion 7 manifest inspection

The verifier must reject an incomplete filesystem inspection. Marker discovery,
either manifest walk (including sorting), and manifest comparison now stop with
exit 3 and a diagnostic on error. They must not report a clean installation
from an empty or partial listing. Fetch failures already stop with exit 1.

## Reproduction

On Debian GNU/Linux 13.7, x86_64, run:

```sh
bash scripts/tests/verify-install-inspection.test.sh
```

This offline fixture drives the actual verifier with a small local tarball. It
injects failures through PATH wrappers; it does not install a published release.
Eight cases cover a clean fixture, a real file written outside the prefix,
marker discovery, before/after scans, sorting, comparison, and fetch failure.

Before the fix, using the verifier from PR #38 (`bc5e8f7`):

```sh
git show bc5e8f7:scripts/verify-install.sh > /tmp/verify-install-before.sh
VERIFY_INSTALL_SCRIPT=/tmp/verify-install-before.sh bash scripts/tests/verify-install-inspection.test.sh
```

Exit 1: five regression cases failed. Marker, after-scan and sort failures
incorrectly returned 0. Before-scan and comparison failures returned 1 but
incorrectly blamed outside-prefix paths instead of rejecting inspection.
After the fix, all eight cases pass (suite exit 0). The clean fixture exits 0;
the outside-prefix write and fetch failure exit 1; inspection faults exit 3.
The original `scripts/tests/verify-install.test.sh` remains unchanged and still
checks the published sharpie release and the aliased TMPDIR regression.

## Unresolved limitations; no new platform acceptance

This is fail-closed hardening, not a solution to host activity attribution.
The gate observes new path names under HOME, /usr/local/bin, /usr/local/lib and
/opt. The harness scratch exclusion remains. The former name-based .cache, .npm
and .git exclusions have been removed (see the boundary regression below). It does not detect changes to existing
file contents (except the separately checked shell profiles), transient files
created and removed between scans, or paths outside those roots. The scratch
exclusion also means this is not a sandbox preventing arbitrary writes.

Run 36268568557 at c84974e passed Linux but failed manifest check 5 on Windows
and both macOS architectures. Cache-looking names are not proof that another
process wrote them. A before/after listing cannot identify the writer. Do not
turn these failures green by filtering those names, retrying until quiet, or
scanning only a redirected HOME while dropping the original observable roots.
A real isolation boundary or OS process-attributed write evidence is still
required, with injected escape writes proving that it catches real violations.
Inspection denied by OS permissions is a failed gate, not an empty manifest.

Windows and both macOS jobs have not run with this patch. Published/observed
digests and clean image versions are therefore **not available for this patch**.
The offline fixture does not satisfy any platform row. Platform verification
must record actual image/version, exact command, observed and published digest,
and exit status independently. Johnny owns final review; the parent task owns
merge/re-dispatch and four green rows on one main commit. Do not publish or
infer platform success from these fixture results.

The preserved published-release suite was also run on the same Debian 13.7
workspace with `bash scripts/tests/verify-install.test.sh`: four checks passed,
exit 0, including the real symlink alias case. This is a development workspace,
not a clean-machine installation claim. `bash -n` on both changed scripts and
`git diff --check` passed.

## Archive inspection continuation

Archive listing was another incomplete-inspection path: the exit status of
`tar tzf` inside process substitution was lost. Even a detected unsafe member
only incremented the failure count and still reached extraction. The verifier
now saves a successful complete listing before checking member names and stops
with exit 3 before extraction on either error.

On Debian GNU/Linux 13.7 x86_64, reproduce against the accepted `75eb501`:

```sh
git show 75eb501:scripts/verify-install.sh > /tmp/verify-install-75eb501.sh
VERIFY_INSTALL_SCRIPT=/tmp/verify-install-75eb501.sh bash scripts/tests/verify-install-inspection.test.sh
bash scripts/tests/verify-install-inspection.test.sh
bash scripts/tests/verify-install.test.sh
```

The first command sequence's suite exits 1: the partial-listing case returns 0,
and the unsafe-member case returns 1; both wrongly attempt extraction. With
this change all ten cases pass (suite exit 0); both archive faults exit 3 and
neither reaches extraction. The preserved published-release suite passes all
four checks (exit 0). These fixture archives and workspace runs are not native
clean-machine evidence. Archive member names alone do not validate symlink or
hardlink targets; this patch does not claim complete archive containment.
All host-attribution limitations above still apply.

## Name-based manifest blind spots

Directories named .cache, .npm and .git used to be pruned without evidence
about their writer. A real install write into any such existing directory was
invisible. The manifest now prunes only the harness WORK paths and their
identified aliases. It still fails closed if the larger scan is denied or
fails; permission errors must not become a new exclusion list.

On Debian GNU/Linux 13.7 x86_64 (development container, not a clean image):

```sh
git show 676f6a1:scripts/verify-install.sh > "$PAPERCLIP_RUN_SCRATCH_DIR/verify-install-before.sh"
VERIFY_INSTALL_SCRIPT="$PAPERCLIP_RUN_SCRATCH_DIR/verify-install-before.sh" bash scripts/tests/verify-install-inspection.test.sh
bash scripts/tests/verify-install-inspection.test.sh
bash scripts/tests/verify-install.test.sh
bash -n scripts/verify-install.sh scripts/tests/verify-install-inspection.test.sh
git diff --check
```

With the current test file against 676f6a1, the suite exits 1: cache-leak,
npm-leak and git-leak each incorrectly return 0. Each case creates a real file
outside PREFIX, in a directory created before the initial scan. With the fix,
all thirteen cases pass (suite exit 0); those three leaks return 1. The existing
scan, fetch and archive fault cases retain their fail-closed results.

This removes blind spots; it does not solve host-write attribution. More host
activity may now be visible and legitimately keep the gate red. No Windows or
macOS execution, clean OS image, or release digest evidence is claimed by this
fixture. Before criterion 7 can pass, the execution environment still needs a
validated isolation boundary or process-attributed write collection. That
collector must reject startup/inspection failures and lost events, cover child
processes and writes using original absolute paths, and retain injected escape
regressions. Merely redirecting HOME or filtering cache names is insufficient.
