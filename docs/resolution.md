# What `ingot` promises about versions

Pins, locks and ranges, written down. Everything here is pinned by a test:
the requirement grammar and the set algebra by `tests/cases/semver.ws`, the
solver's choices and its three failure sentences by
`tests/cases/ingot_resolution.ws`, and the store's own promises by
`tests/cases/ingot_store.ws` and `tests/cases/ingot_registry.ws`.

A version is [semver 2.0.0](https://semver.org). `1.0.0-rc.1` is below
`1.0.0`, build metadata (`1.0.0+abc`) does not affect ordering, and a leading
zero (`01.2.3`) is refused rather than tidied away.

## Requirements

A dependency's value in `ingot.toml` is a requirement: a set of versions, not a
version.

| Spelling | Means | Note |
|---|---|---|
| `^1.2.3` | `>=1.2.3 <2.0.0` | up to the next version that may break |
| `^0.2.3` | `>=0.2.3 <0.3.0` | below 1.0.0 the *minor* number carries that meaning |
| `^0.0.3` | `>=0.0.3 <0.0.4` | and below 0.1.0 the patch number does |
| `^1.2` | `>=1.2.0 <2.0.0` | |
| `^0.0` | `>=0.0.0 <0.1.0` | |
| `1.2.3` | `>=1.2.3 <2.0.0` | a bare version is `^`; there is no "exactly" by accident |
| `~1.2.3` | `>=1.2.3 <1.3.0` | up to the next minor version |
| `~1` | `>=1.0.0 <2.0.0` | no minor number to hold fixed |
| `=1.2.3` | exactly `1.2.3` | **this is the pin** |
| `>1.0.0`, `>=1.0.0`, `<2.0.0`, `<=2.0.0` | one open side | |
| `*` | any version | |
| `>=1.0.0, <1.5.0` | intersection | comma is "and"; there is no "or" |

Two consequences worth stating out loud:

* **A bare `1.2.3` is not a pin.** It is `^1.2.3`, which is what almost every
  ecosystem means by it and what `ingot add <name>` writes. Pinning is `=1.2.3`
  and is always deliberate.
* **A requirement can be empty.** `>=1.0.0, <1.0.0` is a set with nothing in
  it, and the solver says so rather than quietly choosing something.

### Pre-releases

**A pre-release is never chosen unless a requirement asked for one.** The
ordering stays honest — `1.3.0-rc.1` really is below `1.3.0` and really is
inside `^1.0.0` — but the *choice* is a policy on top of the set algebra, and
it is per package: asking for `a >=1.3.0-rc.1` says nothing about `b`.

```
a published at 1.0.0, 1.2.0, 1.3.0-rc.1

    a = "^1.0.0"        chooses 1.2.0
    a = ">=1.3.0-rc.1"  chooses 1.3.0-rc.1
```

## Choosing

`ingot resolve` runs [PubGrub](https://nex3.medium.com/pubgrub-2fb6470504f)
over the requirements of the whole project, and chooses **one version per
package** — not one per dependent. Within what is allowed it takes the
**newest**, and it **backtracks**: if the newest `a` needs a `b` that nothing
supplies, the solver takes an older `a` rather than reporting a conflict a
different choice would have avoided.

The search is bounded at 20,000 steps. PubGrub terminates; a *provider* need
not, and a tool that hangs is worse than one that says it gave up.

Three things stand outside the solver:

* **A yanked release is not a candidate**, and is still installable. Yanking
  withdraws a version from being *chosen*; a lockfile that already names one
  must keep working.
* **A path or git dependency overrides the registry for the name it supplies.**
  The solver is never offered published versions of a package somebody is
  editing beside their project.
* **A registry release carries its tree hash**, so resolving a registry
  dependency fetches nothing. A git dependency must be fetched during
  `resolve`, because only the fetched tree holds the manifest saying what *it*
  depends on.

## When there is no answer

`ingot resolve` exits **4** and writes the solver's account to **stderr**,
unprefixed. Stdout carries what a verb achieved — here, nothing — so a script
cutting up `resolved<TAB>n` never has to tell that apart from a page of
reasoning.

The three sentences, each pinned by `tests/cases/ingot_resolution.ws`:

*A package nothing supplies:*

```
Because no versions of ghost match >=1.0.0 <2.0.0
and root 0.0.0 depends on ghost >=1.0.0 <2.0.0, version solving failed.
```

*A range no published version is in:*

```
Because no versions of a match >=9.0.0 <10.0.0
and root 0.0.0 depends on a >=9.0.0 <10.0.0, version solving failed.
```

*Two requirements that cannot both hold:*

```
Because root 0.0.0 depends on a >=1.0.0 <1.1.0
and root 0.0.0 depends on a >=1.2.0 <1.3.0, version solving failed.
```

A conflict deeper than the root is reported as the chain of derivations that
produced it, in the same form, ending in `version solving failed.`

Two failures that are *not* the solver's, and say so differently:

```
`<name>` is asked for and nothing supplies it
`<name>` is not in <registry>
```

The first is a dependency with no source at all; the second is a registry that
does not hold the name. Both exit 4. Both are on stderr under the `ingot: `
prefix, which is also what progress uses — `ingot --quiet` leaves the message
and drops the progress.

## The lockfile

`ingot.lock` records what was chosen. It is TOML, written and read through
`std/toml` in both directions.

```toml
version = 1
manifest = "0ada8b64…"          # the digest of the ingot.toml this came from

[[package]]
name = "quantydb/client"
version = "0.1.0"
source = "reg+quantydb/client@0.1.0"
tree = "sha256:a00f3866…"
dependencies = []
```

**The lock records the manifest's digest, not its timestamp.** Deciding
staleness by modification time is what a package manager must not do: a
checkout does not preserve them, two machines do not agree about them, and git
will hand somebody a manifest older than the lock beside it.

**Only `resolve` changes what is chosen.** `add` and `remove` edit the manifest
and say `stale<TAB>ingot.lock`; they do not re-resolve. `install` refuses a
lock whose `manifest` digest is not this manifest's:

```
ingot.lock does not describe this ingot.toml -- run `ingot resolve`
```

**A re-resolve is not minimal.** `ingot resolve` solves from the manifest
alone, so adding one dependency and re-resolving may move others within their
ranges. The lockfile is what holds a build still, and nothing but `resolve`
moves it. `ingot update` re-fetches the registry index; it is `resolve` that
then acts on it.

**`resolve` removes `ingot.env`.** That file names store directories, and a new
resolution names new ones — while the old entries are still there holding what
they always did, so a compiler reading a stale environment would build against
the previous versions and say nothing. It is removed rather than rewritten,
because where things land is `install`'s answer.

**A published version is never edited.** `store.remembered` maps a source
string to a tree digest and never invalidates it, so `reg+acme/json@1.2.0` must
name one tree for ever. A mistake is a new version; a withdrawal is
`yanked = true`. An index does change, which is why `ingot update` exists.

## `ingot verify` answers with its exit status

So a script can ask without reading a word of output.

| Exit | Means | Reproduced by |
|---|---|---|
| 0 | ready | everything installed and current |
| 1 | needs installing | the lock is current, a store entry is missing |
| 2 | needs resolving | no lockfile, or its `manifest` digest is not this manifest's |
| 3 | broken | no manifest, or something unreadable |

`resolve`, `install` and every other verb exit **4** when they could not be
carried out at all — distinct from `verify`'s answers, which are about a
project rather than about a mistake.

## What is not promised

* **No `--locked` or `--offline` flag on `resolve`.** Reproducing a build is
  `install`, which uses the lockfile and nothing else. A registry directory
  named by `INGOT_REGISTRY` is used where it lies and never fetched, which is
  what an offline or a private registry is.
* **No optional, dev-only or feature-gated dependencies.** Every dependency in
  a manifest is a dependency.
* **No version requirement on the compiler itself.** That is `sharpie`'s
  `wsharp-toolchain.toml`, not `ingot.toml`.
