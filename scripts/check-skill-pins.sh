#!/usr/bin/env bash
# Install the stock fleet, pinned skills included, from GitHub into a
# throwaway RUPU_HOME, then validate every installed skill. Fails when a
# pin does not resolve or a skill does not validate. Needs network.
#
#   scripts/check-skill-pins.sh <path/to/rupu>
set -euo pipefail
rupu="$(cd "$(dirname "${1:?usage: check-skill-pins.sh <path/to/rupu>}")" && pwd)/$(basename "$1")"
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
export RUPU_HOME="$home"
cd "$home"
"$rupu" fleet install
"$rupu" skill validate --all
echo "check-skill-pins: ok"
