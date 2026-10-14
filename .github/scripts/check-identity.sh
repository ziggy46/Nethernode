#!/usr/bin/env bash
# Fails if any commit in the range was made by someone other than the
# project identity, or carries Co-authored-by / Signed-off-by trailers.
#
#   .github/scripts/check-identity.sh [<range>]     (default: the whole history)
set -euo pipefail

readonly NAME="NetherNode"
readonly EMAIL="nethernode@noreply.invalid"
range="${1:-HEAD}"

bad=0
while IFS=$'\t' read -r sha an ae cn ce; do
  if [[ "$an" != "$NAME" || "$ae" != "$EMAIL" || "$cn" != "$NAME" || "$ce" != "$EMAIL" ]]; then
    echo "::error::$sha is by '$an <$ae>' (committed by '$cn <$ce>'), not '$NAME <$EMAIL>'"
    bad=1
  fi
  if git show -s --format=%B "$sha" | grep -qiE '^(co-authored-by|signed-off-by):'; then
    echo "::error::$sha has a Co-authored-by or Signed-off-by trailer"
    bad=1
  fi
done < <(git log --format=$'%H\t%an\t%ae\t%cn\t%ce' "$range")

if [[ "$bad" -eq 0 ]]; then
  echo "All commits in $range use $NAME <$EMAIL> with no trailers."
fi
exit "$bad"
