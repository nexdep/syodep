#!/usr/bin/env bash
# Make sure the rolling `continuous` prerelease exists and points at this commit,
# so a platform publisher can upload its asset into it.
#
# Both publish-continuous-linux and publish-continuous-windows call this, and
# they are deliberately not ordered against each other: the AppImage is
# published as soon as the Linux build finishes, the Windows zip several minutes
# later. Every step here therefore has to be idempotent and survive the other
# job doing the same thing at the same moment -- notably `gh release create`,
# which one of the two callers loses outright on a repository that has no
# `continuous` release yet.
#
# Stale runs never reach this script: each caller first checks that GITHUB_SHA
# is still origin/main, so the tag can only ever move forward.
set -euo pipefail

: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GITHUB_SHA:?GITHUB_SHA is required}"

tag=continuous
title="syodep continuous"

notes_file="$(mktemp)"
trap 'rm -f "$notes_file"' EXIT

# Deliberately free of per-commit detail. The two assets are published
# independently and can be built from different commits, so any single
# "Commit:" line would be wrong for one of them, and a read-modify-write of a
# per-platform line would race between the two callers. Both callers write this
# same body instead, which makes the edit below idempotent whoever gets there
# first. The per-asset commit stays discoverable from the binary itself.
cat > "$notes_file" <<'EOF'
Rolling build from main.

The Linux and Windows assets are published independently, as soon as each
platform finishes building, so they may come from different commits.
Run `syodep --version` to see the commit an asset was built from.
EOF

git tag -f "$tag" "$GITHUB_SHA"
git push origin "refs/tags/$tag" --force

if ! gh release view "$tag" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
    # The other publisher may create it between that check and this call, which
    # is not an error. The result is verified below rather than trusted here.
    gh release create "$tag" \
        --repo "$GITHUB_REPOSITORY" \
        --verify-tag \
        --title "$title" \
        --notes-file "$notes_file" \
        --prerelease || true
fi

if ! gh release view "$tag" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
    echo "the $tag release does not exist and could not be created" >&2
    exit 1
fi

gh release edit "$tag" \
    --repo "$GITHUB_REPOSITORY" \
    --title "$title" \
    --notes-file "$notes_file" \
    --prerelease

# A rolling prerelease must never displace the newest tagged release as the
# repository's "Latest" pointer.
release_id="$(gh api "repos/${GITHUB_REPOSITORY}/releases/tags/${tag}" --jq .id)"
gh api --method PATCH "repos/${GITHUB_REPOSITORY}/releases/${release_id}" \
    -f make_latest=false \
    --silent
