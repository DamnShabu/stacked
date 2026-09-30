#!/usr/bin/env bash
# Apply this repository's GitHub settings and label scheme. Safe to re-run:
# every step checks what is already there and changes only what differs.
#
# Needs `gh`, logged in as somebody with admin rights on the repository.
#
#   scripts/setup-repo.sh                  # DamnShabu/stacked
#   REPO=someone/fork scripts/setup-repo.sh
#
# **Nothing here deletes a label.** A label in .github/labels.yml with a
# `from:` is renamed from the old name with `gh label edit --name`, which keeps
# it on every issue that already carries it. Delete the old one by hand, if at
# all, once nothing uses it.
#
# The ruleset is deliberately narrow; see the comment above it before adding
# to it.
set -euo pipefail

REPO="${REPO:-DamnShabu/stacked}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

command -v gh >/dev/null || { echo "gh is not installed" >&2; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "gh is not logged in: run gh auth login" >&2; exit 1; }

say() { printf '==> %s\n' "$*"; }

# --- Repository features -----------------------------------------------------

say "Discussions and auto-merge"
gh repo edit "$REPO" --enable-discussions --enable-auto-merge

say "Private vulnerability reporting"
gh api -X PUT "repos/$REPO/private-vulnerability-reporting" --silent

say "Dependabot alerts and security updates"
gh api -X PUT "repos/$REPO/vulnerability-alerts" --silent
gh api -X PUT "repos/$REPO/automated-security-fixes" --silent

# --- Labels ------------------------------------------------------------------
#
# .github/labels.yml is read with awk rather than a YAML parser so this runs
# on a machine with neither yq nor Python. That holds only while the file keeps
# its present shape: one `- name:` per label, followed by optional `from:`,
# `color:` and `description:` lines.

say "Labels from .github/labels.yml"
declare -A have=()
while IFS= read -r name; do have["$name"]=1; done \
  < <(gh label list --repo "$REPO" --limit 1000 --json name -q '.[].name')

unquote() { sed -E 's/^"(.*)"$/\1/' <<<"$1"; }

apply_label() {
  local name="$1" from="$2" color="$3" desc="$4"
  [ -n "$name" ] || return 0
  if [ -n "${have[$name]:-}" ]; then
    gh label edit "$name" --repo "$REPO" --color "$color" --description "$desc"
    if [ -n "$from" ] && [ -n "${have[$from]:-}" ]; then
      echo "   note: both '$from' and '$name' exist; '$from' left alone for a person to merge"
    fi
  elif [ -n "$from" ] && [ -n "${have[$from]:-}" ]; then
    echo "   rename '$from' -> '$name'"
    gh label edit "$from" --repo "$REPO" --name "$name" --color "$color" --description "$desc"
    unset "have[$from]"
  else
    echo "   create '$name'"
    gh label create "$name" --repo "$REPO" --color "$color" --description "$desc"
  fi
  have["$name"]=1
}

name="" from="" color="" desc=""
while IFS= read -r line; do
  case "$line" in
    "- name: "*)
      apply_label "$name" "$from" "$color" "$desc"
      name="$(unquote "${line#- name: }")" from="" color="" desc="" ;;
    "  from: "*)        from="$(unquote "${line#  from: }")" ;;
    "  color: "*)       color="$(unquote "${line#  color: }")" ;;
    "  description: "*) desc="$(unquote "${line#  description: }")" ;;
  esac
done < "$ROOT/.github/labels.yml"
apply_label "$name" "$from" "$color" "$desc"

# --- Default-branch ruleset ----------------------------------------------------
#
# Blocks force pushes to `main` and deleting it, for everybody, admins
# included. That is all, and each omission is deliberate:
#
# - **No required review.** The project has one maintainer, and its release
#   process pushes straight to `main` (CLAUDE.md). A required approval with
#   admins included would stop that.
# - **No required status checks.** `test` is switched off on GitHub, and every
#   workflow that does run on pull requests (`Native packages`, `Flatpak`) is
#   path-filtered. A required check that a pull request never triggers
#   leaves it unmergeable forever, so a docs-only change could not land.
#   Re-enable `test` -- it has no path filter -- and add its two matrix jobs
#   here once it is green on `main`.
# - **No linear history.** Pull requests here are merged with merge commits.

say "Ruleset 'main: history'"
ruleset='{
  "name": "main: history",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [ { "type": "deletion" }, { "type": "non_fast_forward" } ]
}'
id="$(gh api "repos/$REPO/rulesets" -q '.[] | select(.name == "main: history") | .id')"
if [ -n "$id" ]; then
  gh api -X PUT "repos/$REPO/rulesets/$id" --input - --silent <<<"$ruleset"
else
  gh api -X POST "repos/$REPO/rulesets" --input - --silent <<<"$ruleset"
fi

say "Done"
