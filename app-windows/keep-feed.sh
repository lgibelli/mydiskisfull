#!/bin/bash
# Keeps the Windows app's update feed on the draft release <tag>: installed
# Windows copies read latest.json from the latest *published* release, so a
# release without the Windows installers (a Mac-only one) would hide the last
# Windows update from them, and they'd say they're up to date. A draft whose
# feed is its own (for its version: the Windows installers built for it)
# keeps it; any other gets the latest release's copy, which still names
# that release's installers (a copy taken earlier is taken again, as a newer
# release may have been published since). Both release paths run it
# (app/release.sh --upload, from a Mac and in release.yml). A failure to
# read either release stops it, rather than leave a draft without the feed.
#
#   app-windows/keep-feed.sh <tag> [owner/repo]
set -euo pipefail

die() { echo "keep-feed.sh: $*" >&2; exit 1; }
[ $# -ge 1 ] || die "usage: keep-feed.sh <tag> [owner/repo]"
tag=$1
repo=${2:-lgibelli/mydiskisfull}
command -v jq >/dev/null || die "jq is needed (it's in macOS 15 and later; or brew install jq)"

# The names of a release's files (the latest published one without a tag).
assets() { gh release view "$@" --repo "$repo" --json assets --jq '.assets[].name'; }

# Every installer the feed <file> names is a file of this repository's
# releases: of the draft <tag> itself (its own feed: anything missing stops
# it), or of a published, not pre-, release. 1 if that one is known to be
# withdrawn (gone, a draft or a prerelease again, or without the file); a
# release that can't be read stops it.
names_installers() {
    local urls url rest from file found err
    # Each platform whole: the updater refuses a feed with one that isn't.
    jq -e '(.platforms | type) == "object" and (.platforms | length > 0)
        and all(.platforms[]; (.url | type) == "string" and (.signature | type) == "string")' \
        "$1" >/dev/null || die "$1 has a platform without its installer or signature"
    urls=$(jq -er '.platforms[] | .url' "$1") || die "$1 names no installers"
    while read -r url; do
        case $url in
            "https://github.com/$repo/releases/download/"*/*) ;;
            *) die "latest.json names $url, not one of this repository's releases" ;;
        esac
        rest=${url#"https://github.com/$repo/releases/download/"}
        from=${rest%%/*}
        file=${rest#*/}
        if [ "$from" = "$tag" ]; then
            assets "$tag" | grep -qxF "$file" || die "$tag's feed names $file, which it doesn't have"
            continue
        fi
        # Drafts aren't found by tag: gone or a draft is a 404.
        if ! found=$(gh api "repos/$repo/releases/tags/$from" \
            --jq 'if .draft or .prerelease then "withdrawn" else ([.assets[].name] | join("\n")) end' \
            2>"$dir/err"); then
            err=$(cat "$dir/err")
            case $err in
                *"HTTP 404"*) echo "latest.json names $from, which is gone"; return 1 ;;
                *) die "couldn't read the release $from: $err" ;;
            esac
        fi
        if [ "$found" = withdrawn ]; then
            echo "latest.json names $from, which is a draft or a prerelease again"
            return 1
        fi
        grep -qxF "$file" <<<"$found" || { echo "latest.json names $file, which $from no longer has"; return 1; }
    done <<<"$urls"
}

# Only a draft: a published release's feed is what installed copies read
# (replacing it would take it away a moment, or for good if the upload
# failed).
is_draft() {
    [ "$(gh release view "$tag" --repo "$repo" --json isDraft --jq .isDraft)" = true ]
}
is_draft || die "$tag isn't a draft: its feed is left as it is"
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
draft=$(assets "$tag") || die "couldn't read the release $tag"
if grep -qx latest.json <<<"$draft"; then
    gh release download "$tag" --repo "$repo" --pattern latest.json --dir "$dir/draft" \
        || die "couldn't download $tag's latest.json"
    if [ "$(jq -r .version "$dir/draft/latest.json")" = "${tag#v}" ]; then
        names_installers "$dir/draft/latest.json" \
            || die "$tag's own feed names a release that was withdrawn"
        echo "$tag has its own Windows update feed: kept."
        exit 0
    fi
fi
# None published yet (a 404) has no feed; any other failure stops it.
if ! latest=$(gh api "repos/$repo/releases/latest" --jq '.assets[].name' 2>"$dir/err"); then
    case $(cat "$dir/err") in
        *"HTTP 404"*) latest= ;;
        *) die "couldn't read the latest published release: $(cat "$dir/err")" ;;
    esac
fi
if ! grep -qx latest.json <<<"$latest"; then
    # A copy taken earlier, from a release since withdrawn: it would offer
    # that update again.
    if grep -qx latest.json <<<"$draft"; then
        is_draft || die "$tag was published meanwhile: its feed is left as it is"
        gh release delete-asset "$tag" latest.json --repo "$repo" --yes
        echo "$tag's copied Windows update feed is removed: the latest release has none."
    else
        echo "The latest published release has no Windows update feed: nothing to keep."
    fi
    exit 0
fi
gh release download --repo "$repo" --pattern latest.json --dir "$dir" \
    || die "couldn't download the latest release's latest.json"
jq -e '(.version | type) == "string" and (.platforms | type) == "object"
    and (.platforms | length > 0)' "$dir/latest.json" >/dev/null \
    || die "the latest release's latest.json isn't a feed"
if ! names_installers "$dir/latest.json"; then
    # Withdrawn: not offered again (a copy taken earlier goes too).
    if grep -qx latest.json <<<"$draft"; then
        is_draft || die "$tag was published meanwhile: its feed is left as it is"
        gh release delete-asset "$tag" latest.json --repo "$repo" --yes
        echo "$tag's copied Windows update feed is removed: the release it named was withdrawn."
    else
        echo "The latest release's Windows update feed names a withdrawn release: not kept."
    fi
    exit 0
fi
is_draft || die "$tag was published meanwhile: its feed is left as it is"
gh release upload "$tag" --repo "$repo" --clobber "$dir/latest.json"
echo "$tag keeps the Windows update feed of $(jq -r .version "$dir/latest.json")."
