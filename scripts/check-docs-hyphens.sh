#!/bin/sh
# Check public Markdown sources. Pass additional generated documentation paths
# as arguments when validating a documentation build.
set -eu
cd "$(dirname "$0")/.."
if [ "$#" -eq 0 ]; then
    set -- README.md COMPATIBILITY.md LIMITATIONS.md docs/defects.md \
        docs/processes.md docs/resolution.md soak/README.md \
        tests/conformance/README.md tests/harness/README.md
fi
perl -ne '
    if (/\xE2\x80[\x93\x94]/) {
        print "$ARGV:$.: use ASCII hyphens instead of en/em dashes\n";
        $bad = 1;
    }
    close ARGV if eof;
    END { exit($bad ? 1 : 0) }
' "$@"
