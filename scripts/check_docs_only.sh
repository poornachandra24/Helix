#!/usr/bin/env bash
set -euo pipefail

BASE_REF="${1:-}"
if [ -z "$BASE_REF" ]; then
  echo "Usage: $0 <base_ref>"
  exit 1
fi

echo "Comparing HEAD against base reference: $BASE_REF"

# Resolve base reference to ensure it exists locally
if ! git cat-file -e "$BASE_REF" 2>/dev/null; then
  echo "Reference $BASE_REF not found locally. Fetching from origin..."
  git fetch origin "$BASE_REF" --depth=1 || true
fi

# Get list of changed files
CHANGED_FILES=$(git diff --name-only "$BASE_REF"...HEAD)

if [ -z "$CHANGED_FILES" ]; then
  echo "No files changed."
  echo "docs_only=true"
  exit 0
fi

# Check each changed file
for file in $CHANGED_FILES; do
  # If the file is not a Rust file, it must be a doc/markdown file or ignored
  if [[ ! "$file" =~ \.rs$ ]]; then
    # Allow .md, docs/, LICENSE, README, .agents.md, etc.
    if [[ "$file" =~ \.md$ ]] || [[ "$file" =~ ^docs/ ]] || [[ "$file" == "LICENSE" ]] || [[ "$file" == "ROADMAP.md" ]] || [[ "$file" == "CHANGELOG.md" ]] || [[ "$file" == "CHECKLIST.md" ]] || [[ "$file" == ".agents.md" ]]; then
      continue
    else
      echo "Non-documentation file changed: $file"
      echo "docs_only=false"
      exit 0
    fi
  fi

  # If it is a Rust file, check if the diff contains any actual code changes.
  # We inspect every line added (+) or removed (-) in the diff (ignoring hunk headers, metadata, etc.).
  # We check if those lines (minus the leading +/- and whitespace) start with //, /*, *, or are empty.
  DIFF_LINES=$(git diff -U0 "$BASE_REF"...HEAD -- "$file" | grep -E '^[+-][^[+-]]' || true)

  while IFS= read -r line; do
    [ -z "$line" ] && continue
    # Strip leading + or -
    content="${line:1}"
    # Trim leading whitespace
    trimmed="${content#"${content%%[![:space:]]*}"}"
    
    # If the trimmed line is not empty and doesn't start with comment markers, it is a code change
    if [ -n "$trimmed" ]; then
      if [[ ! "$trimmed" =~ ^// ]] && [[ ! "$trimmed" =~ ^/\* ]] && [[ ! "$trimmed" =~ ^\* ]] && [[ ! "$trimmed" =~ ^\*/ ]]; then
        echo "Code change detected in $file at line: $trimmed"
        echo "docs_only=false"
        exit 0
      fi
    fi
  done <<< "$DIFF_LINES"
done

echo "All changes are documentation or comments only."
echo "docs_only=true"
exit 0
