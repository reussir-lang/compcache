#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export DOTNET_CLI_HOME="$PWD/.tools/dotnet-home"
export NUGET_PACKAGES="$PWD/.tools/nuget"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
export DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
dotnet tool restore
cd model
dotnet p compile
dotnet p check -tc tcSafety -s "${P_SCHEDULES:-10000}" --seed 1729
dotnet p check -tc tcRecovery -s 100 --seed 1729
rm -f PCheckerOutput/BugFinding/Cache_0_0.txt
negative_log="$(mktemp)"
trap 'rm -f "$negative_log"' EXIT
if dotnet p check -tc tcUnsafePrune -s 10000 --seed 1729 >"$negative_log" 2>&1; then
  echo 'ERROR: unsafe-prune negative control was not detected' >&2
  exit 1
fi
# Do not mistake a toolchain/infrastructure failure for a successful negative test.
cat "$negative_log"
grep -Eq 'Assertion Failed:.*A reader must retain a Ready runtime' PCheckerOutput/BugFinding/Cache_0_0.txt
