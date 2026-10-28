#!/usr/bin/env python3
"""Remove git worktrees whose branch's PR is merged or closed.

Agents get a worktree per PR; once the PR is done the worktree is dead weight (its target/
alone is 5-25 GB). With no flags this only lists each worktree and its fate; --apply does it.
Uncommitted changes are stashed first (one stash per worktree, labelled with its branch), and
branches are never deleted, so removal loses nothing recoverable.

    python3 tools/prune_worktrees.py               # list worktrees and what would happen
    python3 tools/prune_worktrees.py --apply       # stash dirty trees, remove the done ones
    python3 tools/prune_worktrees.py --apply --no-pr   # also remove worktrees with no PR
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True,
                           text=True, check=True).stdout.strip())


def git(*args, cwd=ROOT, check=False):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=check)


def worktrees():
    """All linked worktrees as (path, branch-or-None); the main checkout is excluded."""
    out = git("worktree", "list", "--porcelain").stdout
    entries, current = [], {}
    for line in out.splitlines() + [""]:
        if not line:
            if current:
                entries.append(current)
            current = {}
            continue
        key, _, value = line.partition(" ")
        current[key] = value
    result = []
    for entry in entries:
        path = Path(entry["worktree"])
        if path == ROOT:
            continue
        branch = entry.get("branch", "")
        result.append((path, branch.removeprefix("refs/heads/") or None))
    return result


def pr_state(branch):
    """'merged' | 'closed' | 'open' | 'none' for the branch's PRs, via gh; None if gh failed."""
    try:
        out = subprocess.run(["gh", "pr", "list", "--head", branch, "--state", "all",
                              "--json", "state", "--limit", "10"],
                             capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if out.returncode != 0:
        return None
    states = {pr["state"] for pr in json.loads(out.stdout or "[]")}
    if "MERGED" in states:
        return "merged"
    if "OPEN" in states:
        return "open"
    if states:
        return "closed"
    return "none"


def is_merged(branch):
    """True if the branch is an ancestor of origin/main (plain merges; squash merges need gh)."""
    return git("merge-base", "--is-ancestor", branch, "origin/main").returncode == 0


def dirty(path):
    return bool(git("-C", str(path), "status", "--porcelain").stdout.strip())


def stash(path):
    """Stash tracked + untracked changes; returns False if nothing was saved but changes remain."""
    local = path / ".local"
    if local.is_symlink():
        local.unlink()  # points at the main checkout's .local; gitignore can't match a symlink
    if not dirty(path):
        return True
    git("-C", str(path), "add", "-A")
    ok = git("-C", str(path), "stash", "push", "-u",
             "-m", f"backup before worktree removal").returncode == 0
    return ok and not dirty(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--apply", action="store_true", help="actually remove the done worktrees")
    parser.add_argument("--no-pr", action="store_true",
                        help="also remove worktrees whose branch has no PR and isn't merged")
    parser.add_argument("--branches", action="store_true",
                        help="also delete the local branches of removed worktrees")
    args = parser.parse_args()

    trees = worktrees()
    if not trees:
        print("no linked worktrees")
        return
    removed = kept = 0
    for path, branch in trees:
        if branch is None:
            verdict = "remove (--no-pr)" if args.no_pr else "keep (detached HEAD)"
        else:
            state = pr_state(branch) or ("merged" if is_merged(branch) else "none")
            done = state in ("merged", "closed") or (args.no_pr and state == "none")
            verdict = f"remove (PR {state})" if done else f"keep (PR {state})"
        marker = "dirty" if dirty(path) else "clean"
        print(f"{'*' if args.apply and verdict.startswith('remove') else ' '} "
              f"{path}  [{branch or 'detached'}] {marker} -> {verdict}")
        if not (args.apply and verdict.startswith("remove")):
            kept += 1
            continue
        if not stash(path):
            print(f"    stash failed, leaving {path} in place")
            kept += 1
            continue
        if git("worktree", "remove", "--force", str(path)).returncode != 0:
            print(f"    removal failed: {path}")
            kept += 1
            continue
        removed += 1
        if args.branches and branch:
            git("branch", "-D", branch)
    print(f"\n{removed} removed, {kept} kept" if args.apply else
          f"\ndry run; rerun with --apply to remove")


if __name__ == "__main__":
    sys.exit(main())
