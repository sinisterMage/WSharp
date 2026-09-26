# The soak log

Criterion 1 of `RELEASE-CRITERIA-1.0.md` asks whether three subjects were
exercised continuously for the freeze window. This directory is the answer's
raw data: one TSV per subject, one row per day, appended by whoever owns that
subject.

```sh
scripts/soak-report.sh                       # the 28-day window ending today
scripts/soak-report.sh --window 7            # a shorter look
scripts/soak-report.sh --end 2026-10-24      # a window that ended in the past
```

It exits 0 only when every subject in `subjects.tsv` has a row for every day of
the window and every one of those rows passed. **A missing row is a failure**, by
design: a soak that stopped reporting is a soak that stopped, and a report that
averaged over the days it happened to have would turn silence into something
that looks like evidence.

## Appending a row

`scripts/soak-report.sh --append` is the only writer, so the format has one
definition. Each owner runs it at the end of their daily job:

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
caught rather than credited to the wrong owner.

Several rows for one subject on one day are expected — a subject run on four
triples writes four. The day counts as reported when at least one row exists,
and fails when **any** row for it failed: a batch that passed on three platforms
and failed on the fourth did not pass.

## Subjects

`subjects.tsv`, so adding one is a diff somebody reviews rather than a change to
the script.

| Subject | Owner | What it is |
|---|---|---|
| `compiler` | Dex | `tests/harness/nightly.sh` once a day — parity across the three execution modes, the fuzz corpus, the pause measurements |
| `ecosystem` | Ash | the `.wsharp` ecosystem check, one full run per day on all four release triples |
| `raython` | Wren | the Raython sample application, a long-running HTTP server |

A subject with no rows is not a subject that is doing fine — it is a `?` for
every day and it fails the gate. That is the intended reading while `raython`
does not exist yet.
