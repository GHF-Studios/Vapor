#!/bin/sh
set -eu

link=1
output=
expect_output=0
for argument do
    if [ "$expect_output" -eq 1 ]; then
        output=$argument
        expect_output=0
        continue
    fi
    case "$argument" in
        -c|-E|-S) link=0 ;;
        -o) expect_output=1 ;;
        -o*) output=${argument#-o} ;;
    esac
done

if [ "$link" -eq 1 ] && [ -n "$output" ]; then
    printf '%s\000' "$@" > "$output.vapor.args"
fi
exec "$VAPOR_MANAGED_ZIG" cc -target "$VAPOR_GIT_BUILD_TARGET" "$@"
