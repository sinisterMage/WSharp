# The soak log

Criterion 1 of `RELEASE-CRITERIA-1.0.md` asks whether three subjects were
exercised continuously for the freeze window. This directory is the answer's
raw data: one TSV per subject, one row per day, appended by the job that
exercises that subject.

```sh
scripts/soak-report.sh                       # the 28-day window ending today
scripts/soak-report.sh --window 7            # a shorter look
scripts/soak-report.sh --end 2026-10-24      # a window that ended in the past
```

The rows the scheduled jobs write are not here: they are appended to the
`soak-log` branch, which holds `soak/<subject>.tsv` and nothing else, so that a
job running every night never puts an unreviewed commit on `main`. Read them
from a checkout of it; the subjects still come from this directory's
`subjects.tsv`, which is what `soak-report.sh` falls back to when the directory
it is pointed at has none:

```sh
git fetch origin soak-log
git worktree add ../soak-log origin/soak-log
scripts/soak-report.sh --dir ../soak-log/soak
```

It exits 0 only when every subject in `subjects.tsv` has a row for every day of
the window and every one of those rows passed. **A missing row is a failure**, by
design: a soak that stopped reporting is a soak that stopped, and a report that
averaged over the days it happened to have would turn silence into something
that looks like evidence.

## Appending a row

`scripts/soak-report.sh --append` is the only writer, so the format has one
definition. Each subject's daily job runs it at the end:

```sh
scripts/soak-report.sh --append \
    --subject compiler --verdict ok \
    --commit "$(git rev-parse HEAD)" \
    --platform x86_64-unknown-linux-gnu \
    --detail '257 cases, 0 diverged, 0 nondeterministic'
```

Six tab-separated fields, no header: date, subject, verdict (`ok` or `fail`),
the full commit SHA, a target triple or `-`, and free-text detail. The subject
field is checked against the file's name, so a row appended to the wrong file is
caught rather than credited to the wrong subject.

Several rows for one subject on one day are expected — a subject run on four
triples writes four. The day counts as reported when at least one row exists,
and fails when **any** row for it failed: a batch that passed on three platforms
and failed on the fourth did not pass. A subject whose line in `subjects.tsv`
names its platforms needs a row from **each** of them every day, and a day that
three of four reported is `-` in the calendar and fails as a missing day does:
"on all four release triples" is part of the gate, not a hope.

## Subjects

`subjects.tsv`, so adding one is a diff somebody reviews rather than a change to
the script: name, what writes its rows, what it is, and optionally the
comma-separated platforms every day must cover.

| Subject | Written by | What it is |
|---|---|---|
| `compiler` | `nightly.yml` | `tests/harness/nightly.sh` once a day — parity across the three execution modes, the fuzz corpus, the pause measurements |
| `ecosystem` | `ecosystem.yml` | `scripts/ecosystem-check.sh`, run by `.github/workflows/ecosystem.yml` once a day on each of the four release triples, each of which must report |
| `raython` | the Raython deployment | the Raython sample application, a long-running HTTP server |

A subject with no rows is not a subject that is doing fine — it is a `?` for
every day and it fails the gate. That is the intended reading until the Raython
sample application is deployed and reporting.
