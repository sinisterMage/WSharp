#!/usr/bin/env perl
# A grammar-aware generator of W# programs.
#
# `fuzz.pl` mutates real programs with a token dictionary. That reaches deep
# parser states cheaply, because it starts from something the parser already
# accepts -- and it is why the README says the harness has no grammar model. Two
# things a mutator structurally cannot do:
#
#   1. **Construct a program the corpus does not resemble.** A mutation of
#      `tests/cases/arithmetic.ws` is a program shaped like `arithmetic.ws`. A
#      `for` loop over a generic struct inside a `catch` block inside a closure
#      is not two bit flips away from anything committed, so no amount of budget
#      finds it.
#   2. **Say what it covered.** For a compiler the interesting measure is which
#      *constructs*, and which *combinations* of constructs, were exercised --
#      not which lines of Rust ran. A mutator cannot report that, because it does
#      not know what it built.
#
# So this builds programs from a grammar, records which production it used and
# which production each one was nested inside, and reports both. Every generated
# program is intended to be **syntactically valid**: what is being looked for is
# not a parse error but the outcomes that are not a diagnostic at all -- a Rust
# panic, a signal, or a program that will not terminate -- reached through
# construct combinations nobody wrote a case for.
#
# Three properties, each a lens rather than a nicety:
#
# **Seed determinism.** A program is a pure function of the seed. The generator
# is the same 31-bit LCG `fuzz.pl` uses, implemented here rather than taken from
# perl's `rand`, so `--seed 1234` is the same program on every machine and every
# perl build. A finding that cannot be replayed from its seed is a rumour.
#
# **Depth is bounded and declared.** Recursion is cut at `--max-depth`, and the
# cut is a *terminal production* rather than a truncation, so a program is never
# half-written. An unbounded generator writes one program and then hangs the
# harness on it.
#
# **Coverage is reported, and it is grammar coverage.** `--coverage` writes one
# row per production with the number of times it was used, and one row per
# (parent, child) pair. A production with a count of 0 across a whole campaign is
# a construct the campaign did not test, which is the sentence a line-coverage
# number cannot produce.
#
# Usage, standalone:
#   tests/harness/generate.pl --seed 1 --count 50 --out target/harness/grammar
#   tests/harness/generate.pl --seed 7 --print            # one program, stdout
#   tests/harness/generate.pl --seed 1 --count 500 --coverage
#
# Used by `fuzz.pl`: `require`d, and `ws_generate($seed, $max_depth)` is called
# with the caller's own seed so a campaign stays reproducible from one number.
#
# What this does **not** do: it has no coverage feedback, so it cannot steer
# toward a construct it has not reached -- coverage here is *reported* after the
# fact, not used to guide generation. And a generated program is not checked
# against a model of what W# should do with it; the oracle is only "did the
# compiler behave, or did it panic, crash or hang". See tests/harness/README.md.

use strict;
use warnings;

# ---------------------------------------------------------------------------
# The generator. Identical to `fuzz.pl`'s on purpose: two halves of one harness
# that disagreed about what a seed means could not hand each other a finding.
# ---------------------------------------------------------------------------
my $STATE = 0;

sub g_seed { $STATE = ($_[0] & 0x7FFFFFFF) || 1; g_rnd(2) for 1 .. 8; return; }

sub g_rnd {
    my ($n) = @_;
    $STATE = (1103515245 * $STATE + 12345) & 0x7FFFFFFF;
    return $n > 0 ? ($STATE >> 7) % $n : 0;
}

sub g_pick { my @xs = @_; return $xs[ g_rnd(scalar @xs) ]; }

# ---------------------------------------------------------------------------
# Coverage accounting.
#
# `%USED` counts productions; `%PAIRS` counts (enclosing production, production)
# so a report can say that `catch_block` was generated 300 times and *never*
# inside `closure`. The combination is the interesting half: a construct in
# isolation usually has a case, and a construct inside another usually does not.
# ---------------------------------------------------------------------------
my %USED;
my %PAIRS;
my $PARENT = 'toplevel';

# Every production wraps its body in this, so accounting cannot be forgotten in
# one arm and drift. `$name` is the grammar's name for the thing, not perl's.
sub prod {
    my ($name, $body) = @_;
    $USED{$name}++;
    $PAIRS{"$PARENT\t$name"}++;
    my $saved = $PARENT;
    $PARENT = $name;
    my $out = $body->();
    $PARENT = $saved;
    return $out;
}

sub reset_coverage { %USED = (); %PAIRS = (); $PARENT = 'toplevel'; return; }

# The grammar's full production list, declared rather than inferred from what a
# run happened to use. This is what makes "count 0" a reportable fact: without a
# declared list, a production the generator can no longer reach simply disappears
# from the report and reads as absent-by-design.
our @PRODUCTIONS = qw(
    program import struct generic_struct subtype fn_decl fn_generic fn_fallible
    const_decl var_decl assign if_stmt while_stmt for_stmt return_stmt
    try_stmt catch_block closure closure_generic call dispatch_call
    binary unary index field_access literal_int literal_float literal_bool
    literal_str literal_hex array_new array_lit optional_wrap error_wrap
    cast print_stmt block spawn worker_call
);

# ---------------------------------------------------------------------------
# Types, names and leaves
# ---------------------------------------------------------------------------
my @INT_TYPES   = qw(i8 i16 i32 i64 u8 u16 u32 u64);
my @SCALARS     = (@INT_TYPES, 'f64', 'bool');
my $NEXT_NAME   = 0;

sub fresh { my ($stem) = @_; $NEXT_NAME++; return "$stem$NEXT_NAME"; }

sub a_type {
    my ($depth) = @_;
    return g_pick(@SCALARS) if $depth <= 0;
    my $r = g_rnd(10);
    return prod('optional_wrap', sub { '?' . a_type($depth - 1) }) if $r == 0;
    return prod('error_wrap',    sub { '!' . a_type($depth - 1) }) if $r == 1;
    return '[]' . g_pick(@SCALARS) if $r == 2;
    return 'str' if $r == 3;
    return g_pick(@SCALARS);
}

# A literal of a type the caller names, so a generated program is not merely
# parseable but plausibly typeable. `0x` and a float are separate productions
# because they are separate lexer paths.
sub a_literal {
    my ($ty) = @_;
    return prod('literal_bool', sub { g_pick('true', 'false') }) if $ty eq 'bool';
    return prod('literal_float', sub {
        my $n = g_rnd(1000);
        my $f = g_rnd(1000);
        return "$n.$f";
    }) if $ty eq 'f64';
    return prod('literal_str', sub {
        # No escape and no quote inside: the lexer's escape handling is
        # `fuzz.pl`'s business, and an unterminated string here would make every
        # program a parse error rather than a typed program.
        my @w = qw(alpha beta gamma delta epsilon zeta);
        return '"' . join('', map { g_pick(@w) } 1 .. (1 + g_rnd(3))) . '"';
    }) if $ty eq 'str';
    return prod('literal_hex', sub { sprintf('0x%x', g_rnd(255)) }) if g_rnd(6) == 0;
    return prod('literal_int', sub { '' . g_rnd(64) });
}

# ---------------------------------------------------------------------------
# Expressions
# ---------------------------------------------------------------------------
# `$scope` maps a name to its type, so a generated expression refers to
# something that exists. A generator that invented names would produce programs
# the compiler rejects for one reason -- an unknown identifier -- and would never
# reach inference at all.
sub an_expr {
    my ($scope, $ty, $depth) = @_;
    my @same = grep { $scope->{$_} eq $ty } sort keys %$scope;

    if ($depth <= 0) {
        return $same[ g_rnd(scalar @same) ] if @same && g_rnd(2);
        return a_literal($ty);
    }

    my $r = g_rnd(12);

    # Arithmetic, on an integer type only: `%` and the bit operators are not
    # defined for every member of `Number`, and a body that used one on an `f64`
    # is a program this generator meant to be well typed.
    if ($r <= 3 && grep { $_ eq $ty } @INT_TYPES) {
        return prod('binary', sub {
            my $op = g_pick('+', '-', '*', '|', '&', '^');
            my $l = an_expr($scope, $ty, $depth - 1);
            my $rr = an_expr($scope, $ty, $depth - 1);
            return "($l $op $rr)";
        });
    }
    if ($r == 4 && $ty eq 'f64') {
        return prod('binary', sub {
            my $op = g_pick('+', '-', '*');
            return '(' . an_expr($scope, $ty, $depth - 1) . " $op "
                 . an_expr($scope, $ty, $depth - 1) . ')';
        });
    }
    if ($r == 5 && $ty eq 'bool') {
        return prod('binary', sub {
            my $op = g_pick('==', '!=', '<', '>', '<=', '>=');
            my $it = g_pick(@INT_TYPES);
            return '(' . an_expr($scope, $it, $depth - 1) . " $op "
                 . an_expr($scope, $it, $depth - 1) . ')';
        });
    }
    if ($r == 6 && grep { $_ eq $ty } @INT_TYPES) {
        # A negation is only well typed on a signed type: `Signed` exists for
        # exactly this reason, and CLAUDE.md says so.
        return prod('unary', sub {
            my $inner = an_expr($scope, $ty, $depth - 1);
            return $ty =~ /^u/ ? "($inner)" : "(-$inner)";
        });
    }
    if ($r == 7 && $ty eq 'bool') {
        return prod('unary', sub { '(!' . an_expr($scope, 'bool', $depth - 1) . ')' });
    }
    if ($r == 8) {
        my @arrays = grep { $scope->{$_} eq "[]$ty" } sort keys %$scope;
        if (@arrays) {
            return prod('index', sub {
                my $arr = $arrays[ g_rnd(scalar @arrays) ];
                # Concatenated rather than interpolated: `"$arr["` is a perl
                # array subscript, which is a compile error in this file and was.
                return $arr . '[' . an_expr($scope, 'i64', 0) . ']';
            });
        }
    }
    if ($r == 9 && grep { $_ eq $ty } @INT_TYPES) {
        # A width conversion, spelled as W# spells it: the *type name* is the
        # call, `i64(v)`, as `abstract_convert_widths.ws` does. This is the one
        # place a generated program most wants to be wrong about signedness, and
        # so the one worth generating a lot of.
        return prod('cast', sub {
            my $from = g_pick(@INT_TYPES);
            return $ty . '(' . an_expr($scope, $from, $depth - 1) . ')';
        });
    }

    return $same[ g_rnd(scalar @same) ] if @same && g_rnd(2);
    return a_literal($ty);
}

# ---------------------------------------------------------------------------
# Statements
# ---------------------------------------------------------------------------
sub a_block {
    my ($scope, $ret, $depth, $indent) = @_;
    return prod('block', sub {
        my $pad = '    ' x $indent;
        my @out;
        my %inner = %$scope;
        for (1 .. 1 + g_rnd(3)) {
            push @out, a_statement(\%inner, $ret, $depth - 1, $indent);
        }
        push @out, "$pad" . 'return ' . an_expr(\%inner, $ret, 1) . ';'
            if $ret ne 'void';
        return join("\n", @out);
    });
}

sub a_statement {
    my ($scope, $ret, $depth, $indent) = @_;
    my $pad = '    ' x $indent;

    # At the floor, a statement that cannot recurse. A truncated `if` would be a
    # syntax error, which would make every deep program a parse error and hide
    # everything this generator exists to find.
    if ($depth <= 0) {
        return prod('print_stmt', sub {
            my $t = g_pick(@INT_TYPES);
            return $pad . 'print(' . an_expr($scope, $t, 0) . ');';
        });
    }

    my $r = g_rnd(10);

    if ($r <= 1) {
        return prod('const_decl', sub {
            my $n = fresh('c');
            my $t = g_pick(@SCALARS);
            my $e = an_expr($scope, $t, $depth - 1);
            $scope->{$n} = $t;
            return "$pad" . "const $n: $t = $e;";
        });
    }
    if ($r == 2) {
        return prod('var_decl', sub {
            my $n = fresh('v');
            my $t = g_pick(@SCALARS);
            my $e = an_expr($scope, $t, $depth - 1);
            $scope->{$n} = $t;
            return "$pad" . "var $n: $t = $e;";
        });
    }
    if ($r == 3) {
        my @vars = grep { /^v/ } sort keys %$scope;
        if (@vars) {
            return prod('assign', sub {
                my $n = $vars[ g_rnd(scalar @vars) ];
                return "$pad$n = " . an_expr($scope, $scope->{$n}, $depth - 1) . ';';
            });
        }
    }
    if ($r == 4) {
        return prod('if_stmt', sub {
            my $c = an_expr($scope, 'bool', $depth - 1);
            my $then = a_block($scope, 'void', $depth - 1, $indent + 1);
            my $out = "$pad" . "if ($c) {\n$then\n$pad}";
            if (g_rnd(2)) {
                my $els = a_block($scope, 'void', $depth - 1, $indent + 1);
                $out .= " else {\n$els\n$pad}";
            }
            return $out;
        });
    }
    if ($r == 5) {
        # A bounded `while`. An unbounded one would make HANG the generator's
        # own doing, and a hang the harness caused is not a finding about W#.
        return prod('while_stmt', sub {
            my $i = fresh('i');
            my $limit = 1 + g_rnd(8);
            my %inner = (%$scope, $i => 'i64');
            my $body = a_block(\%inner, 'void', $depth - 1, $indent + 1);
            return "$pad" . "var $i: i64 = 0;\n"
                 . "$pad" . "while ($i < $limit) {\n$body\n"
                 . "$pad    $i = $i + 1;\n$pad}";
        });
    }
    if ($r == 6) {
        return prod('array_new', sub {
            my $n = fresh('a');
            my $t = g_pick(@SCALARS);
            $scope->{$n} = "[]$t";
            # `array.new` takes the **count only** -- one argument. The element
            # type comes from the annotation, which is the whole point of
            # `err_unpinned_generic.ws`: `array.new(32)` with nothing to pin `T`
            # is the case that fails. So the annotation is not optional here.
            return "$pad" . "const $n: []$t = array.new(" . (1 + g_rnd(8)) . ');';
        });
    }
    if ($r == 7) {
        return prod('closure', sub {
            my $n = fresh('f');
            my $t = g_pick(@INT_TYPES);
            my $p = fresh('p');
            my %inner = (%$scope, $p => $t);
            my $body = an_expr(\%inner, $t, $depth - 1);
            $scope->{$n} = 'fn';
            return "$pad" . "const $n = fn ($p: $t) $t { return $body; };";
        });
    }
    if ($r == 8) {
        my @fns = grep { $scope->{$_} eq 'fn' } sort keys %$scope;
        if (@fns) {
            return prod('call', sub {
                my $f = $fns[ g_rnd(scalar @fns) ];
                return "$pad" . 'print(' . $f . '(' . an_expr($scope, 'i64', 0) . '));';
            });
        }
    }

    return prod('print_stmt', sub {
        my $t = g_pick(@SCALARS);
        return $pad . 'print(' . an_expr($scope, $t, $depth - 1) . ');';
    });
}

# ---------------------------------------------------------------------------
# Declarations
# ---------------------------------------------------------------------------
# These three answer with an **arrayref**, not a two-element list. `prod` calls
# its body in scalar context, so a body returning `($text, $name)` hands back
# only `$name` -- which put a bare `S1` in the generated program where the whole
# struct declaration should have been, and every such program was a syntax error.
# One reference, so the context cannot be lost again.
sub a_struct {
    my ($depth) = @_;
    return prod('struct', sub {
        my $n = 'S' . ++$NEXT_NAME;
        my @fields = map { '    .' . fresh('f') . ': ' . a_type($depth - 1) . ',' }
                     1 .. (1 + g_rnd(3));
        return ["const $n = struct {\n" . join("\n", @fields) . "\n};", $n];
    });
}

sub a_generic_struct {
    my ($depth) = @_;
    return prod('generic_struct', sub {
        my $n = 'G' . ++$NEXT_NAME;
        return ["const $n = struct[T] {\n    ." . fresh('f') . ": T,\n};", $n];
    });
}

sub a_subtype {
    my ($parent, $depth) = @_;
    return prod('subtype', sub {
        my $n = 'Sub' . ++$NEXT_NAME;
        return ["const $n = struct : $parent {\n    ." . fresh('f')
              . ': ' . g_pick(@SCALARS) . ",\n};", $n];
    });
}

sub a_fn {
    my ($depth, $kind) = @_;
    my $name = fresh('fn_');
    if ($kind eq 'generic') {
        return prod('fn_generic', sub {
            my $p = fresh('p');
            return 'fn ' . $name . '[T](' . $p . ': T) T {' . "\n"
                 . '    return ' . $p . ';' . "\n" . '}';
        });
    }
    if ($kind eq 'fallible') {
        return prod('fn_fallible', sub {
            my $p = fresh('p');
            my $t = g_pick(@INT_TYPES);
            return "fn $name($p: $t) !$t {\n"
                 . "    if ($p == 0) { return error.Zero; }\n"
                 . "    return $p;\n}";
        });
    }
    return prod('fn_decl', sub {
        my $t = g_pick(@SCALARS);
        my $p = fresh('p');
        my %scope = ($p => $t);
        my $body = a_block(\%scope, $t, $depth, 1);
        return "fn $name($p: $t) $t {\n$body\n}";
    });
}

# ---------------------------------------------------------------------------
# A whole program
# ---------------------------------------------------------------------------
# Always has exactly one `main`, because `crates/wsharp-cli/tests/cases.rs`'s
# rule -- a file with no `main` is not a case -- is also the rule for anything
# `wsharp run` is pointed at.
sub ws_generate {
    my ($seed, $max_depth) = @_;
    $max_depth = 4 unless defined $max_depth && $max_depth > 0;
    g_seed($seed);
    $NEXT_NAME = 0;

    return prod('program', sub {
        my @parts;
        my %scope;

        # `std/array` is imported whenever an array might be generated, rather
        # than conditionally: an unused import is legal and a missing one is a
        # diagnostic about the harness rather than about W#.
        push @parts, prod('import', sub { 'const array = @import("std/array");' });

        if (g_rnd(3) == 0) {
            my ($text, $n) = @{ a_struct($max_depth) };
            push @parts, $text;
            if (g_rnd(2)) {
                push @parts, @{ a_subtype($n, $max_depth) }[0];
            }
        }
        if (g_rnd(4) == 0) {
            push @parts, @{ a_generic_struct($max_depth) }[0];
        }

        for (1 .. 1 + g_rnd(2)) {
            push @parts, a_fn($max_depth, g_pick('plain', 'generic', 'fallible'));
        }

        my %main_scope;
        my $body = a_block(\%main_scope, 'i64', $max_depth, 1);
        push @parts, "fn main() i64 {\n$body\n}";

        return join("\n\n", @parts) . "\n";
    });
}

# ---------------------------------------------------------------------------
# The coverage report
# ---------------------------------------------------------------------------
sub ws_coverage_report {
    my @lines;
    push @lines, '# grammar coverage';
    push @lines, '';
    push @lines, '| production | uses |';
    push @lines, '|---|---|';
    my $unused = 0;
    for my $p (@PRODUCTIONS) {
        my $n = $USED{$p} || 0;
        $unused++ unless $n;
        push @lines, "| `$p` | $n |";
    }
    push @lines, '';
    push @lines, "Productions never generated: **$unused of "
               . scalar(@PRODUCTIONS) . '**. A production with a count of 0 is a'
               . ' construct this campaign did not test.';
    push @lines, '';
    push @lines, '| enclosing | nested | uses |';
    push @lines, '|---|---|---|';
    for my $k (sort { $PAIRS{$b} <=> $PAIRS{$a} || $a cmp $b } keys %PAIRS) {
        my ($parent, $child) = split /\t/, $k;
        push @lines, "| `$parent` | `$child` | $PAIRS{$k} |";
    }
    return join("\n", @lines) . "\n";
}

sub ws_coverage_counts { return (\%USED, \%PAIRS); }

# ---------------------------------------------------------------------------
# Standalone entry point. Skipped when `require`d, so `fuzz.pl` gets the
# functions and not the command line.
# ---------------------------------------------------------------------------
unless (caller) {
    my ($seed, $count, $out, $print, $coverage, $max_depth) = (1, 0, '', 0, 0, 4);
    while (@ARGV) {
        my $a = shift @ARGV;
        if    ($a eq '--seed')      { $seed      = shift @ARGV }
        elsif ($a eq '--count')     { $count     = shift @ARGV }
        elsif ($a eq '--out')       { $out       = shift @ARGV }
        elsif ($a eq '--max-depth') { $max_depth = shift @ARGV }
        elsif ($a eq '--print')     { $print     = 1 }
        elsif ($a eq '--coverage')  { $coverage  = 1 }
        elsif ($a eq '--help')      {
            print "usage: generate.pl --seed N [--count K --out DIR] [--print] [--coverage]\n";
            exit 0;
        }
        else { print STDERR "generate: unknown argument $a\n"; exit 2 }
    }

    if ($print) {
        print ws_generate($seed, $max_depth);
        print STDERR ws_coverage_report() if $coverage;
        exit 0;
    }

    # Zero programs is not an empty success. The same lesson `conform.sh` learned
    # the hard way: a harness that reports having done nothing must not exit 0,
    # or a mistyped argument reads as a clean campaign.
    if ($count <= 0) {
        print STDERR "generate: --count must be at least 1 (got '$count')\n";
        print STDERR "generate: a campaign that generated nothing is not a clean one\n";
        exit 2;
    }
    if ($out eq '') {
        print STDERR "generate: --count needs --out DIR to write into\n";
        exit 2;
    }

    mkdir $out unless -d $out;
    unless (-d $out) { print STDERR "generate: cannot create $out\n"; exit 2 }

    reset_coverage();
    for my $i (0 .. $count - 1) {
        my $s = $seed + $i;
        my $text = ws_generate($s, $max_depth);
        my $f = "$out/seed$s.ws";
        open my $fh, '>:raw', $f or die "generate: cannot write $f: $!\n";
        print $fh $text;
        close $fh;
    }
    print "generate: $count program(s) into $out, seeds $seed.."
        . ($seed + $count - 1) . "\n";

    if ($coverage) {
        my $f = "$out/coverage.md";
        open my $fh, '>', $f or die "generate: cannot write $f: $!\n";
        print $fh ws_coverage_report();
        close $fh;
        print "generate: coverage at $f\n";
    }
    exit 0;
}

1;
