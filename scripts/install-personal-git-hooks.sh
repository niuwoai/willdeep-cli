#!/usr/bin/env bash
# Opt-in: point this clone/worktree at repo-managed git hooks (personal, not CI).
set -euo pipefail
root="$(git rev-parse --show-toplevel)"
cd "$root"
chmod +x scripts/git-hooks/pre-commit
# Relative to the repository root (works with worktrees sharing one .git config).
git config core.hooksPath scripts/git-hooks
echo "core.hooksPath → scripts/git-hooks"
echo "pre-commit: staged source size (--staged); rustfmt --check on staged .rs only"
echo "To disable on this clone: git config --unset core.hooksPath"
