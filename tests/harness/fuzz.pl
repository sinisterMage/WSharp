#!/usr/bin/env perl
# A mutation fuzzer for the W# front end and for the parsers in `std/`.
#
# What it is looking for is the set of outcomes that are not a diagnostic: a
# Rust panic, a signal, or a program that does not terminate. A compiler that
# refuses garbage with a message is behaving correctly and is not a finding, so
# the great majority of runs here are expected to "fail" and be ignored.
#
# Three properties are deliberate, and each is a lens rather than a nicety:
#
# **Seed determinism.** The generated input is a pure function of the target and
# the seed. The pseudo-random generator is SplitMix64's LCG-shaped cousin
# implemented here rather than perl's `rand`, so `--seed 1234` produces the same
# bytes on every machine and every perl build; a finding you cannot replay is a
# rumour. The one thing that can move an input under a given seed is the seed
# *corpus* changing, which is why every finding is written out as a file rather
# than as a seed number alone.
#
# **Reduction is the fuzzer's job.** An unreduced fuzz crash costs the fixer
# more than it cost the harness. Every finding is shrunk by delta debugging
# against its own signature before it is written out, so what lands in the
# report is usually already the `tests/cases` entry that will guard the fix.
#
# **One finding per cause.** Findings are keyed by signature -- the panic site
# and message, the signal, or "hang" -- so a mutation class that trips the same
# assertion four hundred times is one directory with a count in it.
#
# Usage:
#   tests/harness/fuzz.pl --target check --seed 1 --iterations 500
#   tests/harness/fuzz.pl --target json  --seed 1 --iterations 2000
#   tests/harness/fuzz.pl --replay path/to/input --target check
#
# Targets:
#   check   `wsharp check` over a mutated W# program  (parser + type checker)
#   run     `wsharp run`   over a mutated W# program  (adds lowering and codegen)
#   json    `std/json.parse` over a mutated document, via tests/harness/drivers
#   toml    `std/toml.parse` over a mutated document, via tests/harness/drivers

use strict;
use warnings;
use File::Path qw(make_path);
use File::Temp qw(tempdir);
use Digest::SHA qw(sha256_hex);

# ---------------------------------------------------------------------------
# A generator whose output does not depend on this perl build.
#
# perl's `rand` is `drand48` on most builds and something else on others, and a
# harness whose seeds mean different things on two machines cannot hand a
# finding from one to the other. This is a 31-bit LCG: the constants are the
# ones from the C standard's example, chosen because `1103515245 * 2**31` still
# fits in a 64-bit IV so nothing is ever promoted to a float.
# ---------------------------------------------------------------------------
my $STATE = 0;
sub seed_rng { $STATE = ($_[0] & 0x7FFFFFFF) || 1; rnd(2) for 1 .. 8; }
sub rnd {
    my ($n) = @_;
    $STATE = ($STATE * 1103515245 + 12345) & 0x7FFFFFFF;
    return $n > 0 ? $STATE % $n : 0;
}
sub pick { my @xs = @_; return $xs[rnd(scalar @xs)]; }

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
my $root = $0; $root =~ s{/tests/harness/fuzz\.pl$}{};
$root = '.' if $root eq $0;
# Cargo names the binary `wsharp.exe` on Windows. Prefer whichever exists, so
# `nightly.sh` on a Windows runner fuzzes the compiler rather than reporting
# that there is not one.
my $wsharp = $ENV{WSHARP}
  || (-x "$root/target/debug/wsharp.exe"
      ? "$root/target/debug/wsharp.exe"
      : "$root/target/debug/wsharp");
# `lib.sh` resolves and exports this; a direct invocation of `fuzz.pl` falls
# back to the coreutils name.
my $TIMEOUT_BIN = $ENV{TIMEOUT_BIN} || 'timeout';
my $target     = 'check';
my $seed       = 1;
my $iterations = 200;
my $timeout    = 20;
my $maxseconds = 0;
my $replay     = '';
my $outdir     = '';

while (@ARGV) {
    my $a = shift @ARGV;
    if    ($a eq '--target')      { $target     = shift @ARGV }
    elsif ($a eq '--seed')        { $seed       = shift @ARGV }
    elsif ($a eq '--iterations')  { $iterations = shift @ARGV }
    elsif ($a eq '--timeout')     { $timeout    = shift @ARGV }
    elsif ($a eq '--max-seconds') { $maxseconds = shift @ARGV }
    elsif ($a eq '--replay')      { $replay     = shift @ARGV }
    elsif ($a eq '--out')         { $outdir     = shift @ARGV }
    elsif ($a eq '--help')        { exec "sed -n '2,42p' $0" }
    else { die "fuzz: unknown argument $a\n" }
}
die "fuzz: no compiler at $wsharp\n" unless -x $wsharp;

my %SUFFIX = (check => '.ws', run => '.ws', json => '.json', toml => '.toml');
die "fuzz: unknown target $target\n" unless exists $SUFFIX{$target};

$outdir ||= "$root/target/harness/fuzz-$target-$seed";
make_path("$outdir/findings");
my $work = tempdir(CLEANUP => 1);

# ---------------------------------------------------------------------------
# The seed corpus
#
# Real programs, because a mutator that starts from nothing spends its whole
# budget rediscovering that `fn` is a keyword. For the stdlib parsers the
# corpus is committed under `tests/harness/corpus`, so those seeds travel.
# ---------------------------------------------------------------------------
sub slurp { local $/; open my $fh, '<:raw', $_[0] or return undef; return <$fh>; }
sub spew  { open my $fh, '>:raw', $_[0] or die "fuzz: cannot write $_[0]: $!\n"; print $fh $_[1]; close $fh; }

my @corpus_paths;
if ($target eq 'check' || $target eq 'run') {
    @corpus_paths = (sort glob("$root/tests/cases/*.ws"), sort glob("$root/examples/*.ws"));
} else {
    @corpus_paths = sort glob("$root/tests/harness/corpus/$target/*");
}
my @corpus = grep { defined } map { slurp($_) } @corpus_paths;
die "fuzz: empty seed corpus for $target\n" unless @corpus;

# Tokens worth inserting whole. A bit flip inside an identifier explores very
# little; dropping a `try` or an extra `}` in explores the grammar.
my @DICT_WS = (
    'fn ', 'const ', 'var ', 'return ', 'struct ', 'if ', 'else ', 'while ',
    'for ', 'try ', 'catch ', 'orelse ', 'pub ', '@import("std/str")',
    '@spawn', '@join', 'error.', 'i64', 'u8', 'f64', 'bool', 'str', 'void',
    '{', '}', '(', ')', '[', ']', ';', ',', '.', ':', '?', '!', '|', '=',
    '==', '=>', '..', '0x', '1e999', '-9223372036854775808', '""', '\\u{',
);
my @DICT_JSON = ('{', '}', '[', ']', ',', ':', '"', '\\u', 'null', 'true',
    'false', '0', '-', '.', 'e+', '1e999', "\x{ef}\x{bb}\x{bf}", '\\uD800');
my @DICT_TOML = ('[', ']', '[[', ']]', '=', '.', '"""', "'''", '#', "\n",
    'true', 'inf', 'nan', '1979-05-27T07:32:00Z', '0x', '_', '\\u');
my %DICT = (check => \@DICT_WS, run => \@DICT_WS, json => \@DICT_JSON, toml => \@DICT_TOML);
my $dict = $DICT{$target};

# ---------------------------------------------------------------------------
# Mutation
# ---------------------------------------------------------------------------
sub mutate {
    my ($s) = @_;
    my $len = length $s;
    return $s if $len == 0;
    my $op = rnd(10);
    if ($op == 0) {                                   # flip one bit
        my $i = rnd($len);
        substr($s, $i, 1) = chr(ord(substr($s, $i, 1)) ^ (1 << rnd(8)));
    } elsif ($op == 1) {                              # delete a span
        my $i = rnd($len); my $n = 1 + rnd(16);
        substr($s, $i, $n) = '';
    } elsif ($op == 2) {                              # insert random bytes
        my $i = rnd($len + 1);
        my $n = 1 + rnd(8);
        my $junk = join '', map { chr(32 + rnd(95)) } 1 .. $n;
        substr($s, $i, 0) = $junk;
    } elsif ($op == 3) {                              # duplicate a line
        my @l = split /\n/, $s, -1;
        return $s unless @l;
        my $i = rnd(scalar @l);
        splice @l, $i, 0, $l[$i];
        $s = join "\n", @l;
    } elsif ($op == 4) {                              # delete a line
        my @l = split /\n/, $s, -1;
        return $s unless @l > 1;
        splice @l, rnd(scalar @l), 1;
        $s = join "\n", @l;
    } elsif ($op == 5) {                              # swap two lines
        my @l = split /\n/, $s, -1;
        return $s unless @l > 1;
        my ($i, $j) = (rnd(scalar @l), rnd(scalar @l));
        @l[$i, $j] = @l[$j, $i];
        $s = join "\n", @l;
    } elsif ($op == 6) {                              # splice from another seed
        my $other = $corpus[rnd(scalar @corpus)];
        my $ol = length $other;
        return $s if $ol == 0;
        my $from = rnd($ol); my $n = 1 + rnd($ol - $from);
        substr($s, rnd($len + 1), 0) = substr($other, $from, $n);
    } elsif ($op == 7) {                              # insert a dictionary token
        substr($s, rnd($len + 1), 0) = $dict->[rnd(scalar @$dict)];
    } elsif ($op == 8) {                              # a nesting bomb
        my $c = pick('(', '[', '{');
        substr($s, rnd($len + 1), 0) = $c x (16 + rnd(4096));
    } else {                                          # truncate
        $s = substr($s, 0, rnd($len));
    }
    return $s;
}

sub generate {
    my ($s) = @_;
    my $base = $corpus[rnd(scalar @corpus)];
    my $rounds = 1 + rnd(6);
    $base = mutate($base) for 1 .. $rounds;
    return $base;
}

# ---------------------------------------------------------------------------
# Running one input, and deciding what happened
# ---------------------------------------------------------------------------
# The driver for a stdlib target is built once, not compiled per input.
#
# Compiling `fuzz_json.ws` takes about 2.5 seconds and parsing the document
# takes milliseconds, so recompiling per iteration spends 99% of the budget on
# the compiler while claiming to fuzz the parser. Building once takes the same
# 2.5 seconds for the whole campaign -- 60 inputs went from 157 seconds to
# under 10. It also makes the target the *built* program, which is the backend
# the JIT does not exercise.
my $driver = '';
if ($target eq 'json' || $target eq 'toml') {
    $driver = "$work/fuzz_$target";
    my @build = ($wsharp, 'build', "$root/tests/harness/drivers/fuzz_$target.ws",
                 '-o', $driver);
    my ($status) = spawn_capture(@build);
    if ($status != 0 || !-x $driver) {
        my $err = slurp("$work/stderr") // '';
        die "fuzz: cannot build the $target driver (status $status):\n$err\n";
    }
}

my %COMMAND = (
    check => sub { my $f = shift; return ($wsharp, 'check', $f) },
    run   => sub { my $f = shift; return ($wsharp, 'run', $f) },
    json  => sub { my $f = shift; return ($driver, $f) },
    toml  => sub { my $f = shift; return ($driver, $f) },
);

# fork/exec, with stdout discarded and stderr captured to a file. No shell is
# involved at any point: the input is a file this process wrote, and its name
# is ours, but the rule is worth keeping structural rather than incidental.
sub spawn_capture {
    my (@cmd) = @_;
    my $err = "$work/stderr";
    my $pid = fork();
    die "fuzz: cannot fork: $!\n" unless defined $pid;
    if ($pid == 0) {
        open STDOUT, '>', '/dev/null';
        open STDERR, '>', $err;
        exec { $cmd[0] } @cmd;
        exit 127;
    }
    waitpid($pid, 0);
    return ($?, $err);
}

# What happened, from the wait status and what was written to stderr. Verdicts:
#   OK      a diagnostic, an accepted program, or a W# panic -- all behaviour
#   PANIC   the compiler or runtime panicked in Rust
#   SIGNAL  killed by a signal: a segfault, an abort, an illegal instruction
#   HANG    did not finish inside the timeout
sub decide {
    my ($status, $errfile) = @_;
    my $err = slurp($errfile);
    $err = '' unless defined $err;
    my $sig  = $status & 127;
    my $code = $status >> 8;

    if ($sig == 9) {
        # SIGKILL here is `timeout -s KILL` and nothing else: the harness is
        # the only thing that sends it.
        return ('HANG', 'hang', $err);
    }
    if ($sig != 0) {
        return ('SIGNAL', "signal $sig", $err);
    }
    if ($err =~ /panicked at ([^\n]*)/) {
        my $where = $1;
        my ($msg) = $err =~ /panicked at [^\n]*\n\s*([^\n]*)/;
        $msg = '' unless defined $msg;
        # Addresses and temporary paths differ between runs and must not make
        # two occurrences of one cause look like two causes.
        my $s = "panic $where: $msg";
        $s =~ s/0x[0-9a-f]+/<addr>/g;
        $s =~ s{/tmp/\S+}{<tmp>}g;
        return ('PANIC', $s, $err);
    }
    if ($err =~ /(internal error|not yet implemented|entered unreachable code)/) {
        return ('PANIC', "internal: $1", $err);
    }
    # A W# panic out of a stdlib parser is a finding, because "a parser answers,
    # it does not raise" is the rule those modules are written to: an index out
        # of range inside `std/json` is a defect in the reader, not a verdict
    # about the document. A W# panic out of a *mutated program* is not -- the
    # program did something the language says panics, which is behaviour.
    if (($target eq 'json' || $target eq 'toml') && $err =~ /W# panic: ([^\n]*)/) {
        my $why = $1;
        $why =~ s/\b\d+\b/<n>/g;    # an index in the message is an occurrence
        return ('WSPANIC', "W# panic: $why", $err);
    }
    return ('OK', "exit $code", $err);
}

sub check_input {
    my ($bytes) = @_;
    my $f = "$work/input$SUFFIX{$target}";
    spew($f, $bytes);
    # `lib.sh` resolves this to `timeout` or, on a macOS runner with Homebrew
    # coreutils, `gtimeout`; base macOS has neither, and two of the four release
    # triples are macOS. Read rather than re-derived, so the two halves of the
    # harness cannot disagree about which binary bounds a run.
    my @cmd = ($TIMEOUT_BIN, '-s', 'KILL', $timeout, $COMMAND{$target}->($f));
    my ($status, $errfile) = spawn_capture(@cmd);
    return decide($status, $errfile);
}

# ---------------------------------------------------------------------------
# Reduction: delta debugging against the finding's own signature.
#
# Chunks are lines when there is more than one and bytes when there is not, so
# the same routine reduces a 200-line W# program and a one-line JSON document.
# The property preserved is (verdict, signature): a reduction that trades one
# panic for another is not a reduction of this finding.
# ---------------------------------------------------------------------------
sub reduce {
    my ($bytes, $verdict, $signature) = @_;
    my $holds = sub {
        my ($candidate) = @_;
        return 0 if length($candidate) == 0;
        my ($v, $s) = check_input($candidate);
        return $v eq $verdict && $s eq $signature;
    };

    my $budget = 400;    # reductions are cheap, but a hang costs $timeout each
    for my $pass (1 .. 3) {
        my $mode = (($bytes =~ tr/\n//) > 0) ? 'lines' : 'bytes';
        my @chunks = $mode eq 'lines' ? split(/^/m, $bytes) : split(//, $bytes);
        my $n = scalar @chunks;
        last if $n <= 1;
        my $granularity = 2;
        while ($granularity <= $n && $budget > 0) {
            my $size = int($n / $granularity) || 1;
            my $removed = 0;
            for (my $start = $n - $size; $start >= 0; $start -= $size) {
                last if $budget-- <= 0;
                my @kept = (@chunks[0 .. $start - 1], @chunks[$start + $size .. $#chunks]);
                @kept = @chunks[$start + $size .. $#chunks] if $start == 0;
                my $candidate = join '', @kept;
                next unless $holds->($candidate);
                @chunks = @kept;
                $n = scalar @chunks;
                $removed = 1;
                last;
            }
            $granularity = $removed ? 2 : $granularity * 2;
        }
        my $next = join '', @chunks;
        last if $next eq $bytes;
        $bytes = $next;
    }
    return $bytes;
}

# ---------------------------------------------------------------------------
# Replay mode: one input, no mutation, no reduction.
# ---------------------------------------------------------------------------
if ($replay) {
    my $bytes = slurp($replay) // die "fuzz: cannot read $replay\n";
    my ($v, $s, $err) = check_input($bytes);
    print "verdict:   $v\nsignature: $s\n";
    print "stderr:\n$err\n" if $err ne '';
    exit($v eq 'OK' ? 0 : 1);
}

# ---------------------------------------------------------------------------
# The campaign
# ---------------------------------------------------------------------------
seed_rng($seed);
my $started = time;
my %findings;    # signature -> { count, verdict, input, iteration }
my %verdicts = (OK => 0, PANIC => 0, SIGNAL => 0, HANG => 0);
my $ran = 0;

for my $i (1 .. $iterations) {
    last if $maxseconds && (time - $started) >= $maxseconds;
    my $bytes = generate();
    my ($v, $s) = check_input($bytes);
    $ran++;
    $verdicts{$v}++;
    next if $v eq 'OK';

    if (exists $findings{$s}) {
        $findings{$s}{count}++;
        next;
    }
    # New cause. Reduce it before it is recorded -- a reduction that arrives
    # with the report is usually the `tests/cases` entry that guards the fix.
    my $reduced = reduce($bytes, $v, $s);
    $findings{$s} = {
        count     => 1,
        verdict   => $v,
        input     => $reduced,
        raw       => $bytes,
        iteration => $i,
        shrank    => length($bytes) . ' -> ' . length($reduced),
    };
    my $key = substr(sha256_hex($s), 0, 12);
    make_path("$outdir/findings/$key");
    spew("$outdir/findings/$key/input$SUFFIX{$target}", $reduced);
    spew("$outdir/findings/$key/unreduced$SUFFIX{$target}", $bytes);
    my (undef, undef, $err) = check_input($reduced);
    spew("$outdir/findings/$key/stderr.txt", $err);
    printf STDERR "fuzz: %s at iteration %d (%s) -> findings/%s\n", $v, $i, $s, $key;
}

my $elapsed = time - $started;

# ---------------------------------------------------------------------------
# The report
# ---------------------------------------------------------------------------
open my $md, '>', "$outdir/report.md" or die "fuzz: cannot write the report: $!\n";
my $sha = `git -C $root rev-parse HEAD 2>/dev/null`; chomp $sha; $sha ||= 'unknown';
my $uname = `uname -sm`; chomp $uname;
my $libc = `ldd --version 2>&1 | head -1`; chomp $libc;
print $md <<"HEADER";
# Fuzzing `$target` — seed $seed

| | |
|---|---|
| commit | `$sha` |
| platform | $uname / $libc |
| compiler | `$wsharp` |
| seed | `$seed` |
| iterations run | $ran of $iterations |
| per-run timeout | ${timeout}s |
| duration | ${elapsed}s |
| seed corpus | @{[scalar @corpus]} files |

Replay the campaign:

    tests/harness/fuzz.pl --target $target --seed $seed --iterations $iterations

Replay one finding:

    tests/harness/fuzz.pl --target $target --replay <path to input$SUFFIX{$target}>

## Outcomes

| verdict | count |
|---|---|
| OK (a diagnostic, or an accepted input) | $verdicts{OK} |
| PANIC (Rust panic in the compiler or runtime) | $verdicts{PANIC} |
| SIGNAL (segfault, abort, illegal instruction) | $verdicts{SIGNAL} |
| HANG (no answer within ${timeout}s) | $verdicts{HANG} |

HEADER

if (!%findings) {
    print $md "No finding. $ran inputs produced a diagnostic or an answer.\n";
} else {
    print $md "## Findings — " . scalar(keys %findings) . " distinct causes\n\n";
    for my $s (sort keys %findings) {
        my $f = $findings{$s};
        my $key = substr(sha256_hex($s), 0, 12);
        print $md "### `$key` — $f->{verdict}\n\n";
        print $md "- signature: `$s`\n";
        print $md "- occurrences this campaign: $f->{count}\n";
        print $md "- first seen at iteration $f->{iteration}\n";
        print $md "- reduced: $f->{shrank} bytes\n";
        print $md "- input: `findings/$key/input$SUFFIX{$target}`\n\n";
        print $md "```\n" . substr($f->{input}, 0, 2000) . "\n```\n\n";
    }
}
close $md;

printf "fuzz: %s seed %d — %d inputs, %d OK, %d panic, %d signal, %d hang, %d distinct causes, %ds\n",
    $target, $seed, $ran, $verdicts{OK}, $verdicts{PANIC}, $verdicts{SIGNAL},
    $verdicts{HANG}, scalar(keys %findings), $elapsed;
print "fuzz: $outdir/report.md\n";
exit(%findings ? 1 : 0);
