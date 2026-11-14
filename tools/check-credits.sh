#!/usr/bin/env bash
# Check that everyone credited in data/CONTRIBUTORS and data/TRANSLATORS is
# the author of a commit on the branch being checked.
#
# GitHub's contributor list counts commit authors only. A Co-Authored-By
# trailer shows on the commit but not in that list, so a contribution landed
# by hand under the maintainer's name leaves its contributor out of it. A
# missing author can't be fixed after a release without rewriting history,
# so this runs before every push (tools/git-hooks/pre-push) and in CI.
#
#   tools/check-credits.sh [<commit>]    defaults to HEAD
#
# A person counts when a commit's author address is the
# <id>+<handle>@users.noreply.github.com one GitHub gives them
# (docs/CONTRIBUTING.md#landing-a-contribution says how to land one so).
set -euo pipefail
cd "$(dirname "$0")/.."

rev="${1:-HEAD}"

if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  echo "check-credits: a shallow clone can't show every author; fetch the full history" >&2
  exit 1
fi

authors=$(git log --format='%ae' "$rev" | tr '[:upper:]' '[:lower:]' | sort -u)

missing=0
for file in data/CONTRIBUTORS data/TRANSLATORS; do
  # The file as it is in the commit checked, which is what a push publishes.
  list=$(git show "$rev:$file" 2>/dev/null) || continue
  # One person per line: Display Name <handle> [- note]; # starts a comment.
  while IFS= read -r handle; do
    lower=$(printf '%s' "$handle" | tr '[:upper:]' '[:lower:]')
    if ! printf '%s\n' "$authors" | grep -qE "^([0-9]+\+)?${lower}@users\.noreply\.github\.com$"; then
      printf '%s: %s has no commit authored as them\n' "$file" "$handle" >&2
      missing=1
    fi
  done < <(printf '%s\n' "$list" | grep -v '^[[:space:]]*#' | sed -n 's/.*<\([^>]*\)>.*/\1/p')
done

if [ "$missing" = 1 ]; then
  cat >&2 <<'EOF'

Land their work with them as the commit's author:
  git commit --author="<login> <<id>+<login>@users.noreply.github.com>"
(gh api users/<login> --jq .id). See docs/CONTRIBUTING.md#landing-a-contribution.
EOF
  exit 1
fi
