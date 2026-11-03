#!/bin/sh
# Attach this workflow run's files to a draft release.
#
#   .github/upload-to-draft.sh <release id>
#
# Run by the release jobs of the build workflows, with GH_TOKEN (contents:
# write, actions: read), GITHUB_REPOSITORY and GITHUB_RUN_ID set. Every
# artifact of the run is fetched, checked against its .sha256 file and
# uploaded. Only a draft is ever written to: a published release is refused.
set -eu

id=$1
repo=$GITHUB_REPOSITORY
run=$GITHUB_RUN_ID
case "$id" in
  ''|*[!0-9]*) echo "not a release id: $id" >&2; exit 1 ;;
esac
test "$(gh api "repos/$repo/releases/$id" --jq .draft)" = true || {
  echo "release $id is not a draft; nothing uploaded" >&2
  exit 1
}

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
gh api "repos/$repo/actions/runs/$run/artifacts" --paginate \
  --jq '.artifacts[] | select(.expired | not) | "\(.id) \(.name)"' > "$dir/list"
test -s "$dir/list"
while read -r aid name; do
  case "$name" in
    */*|.*|'') echo "unexpected artifact name: $name" >&2; exit 1 ;;
  esac
  gh api "repos/$repo/actions/artifacts/$aid/zip" > "$dir/$name.download"
  # Files uploaded with archive: false come back as they are; a zipped
  # artifact holds the one file of that name.
  if [ "$(head -c 4 "$dir/$name.download" | od -An -tx1 | tr -d ' ')" = 504b0304 ] \
     && unzip -l "$dir/$name.download" "$name" >/dev/null 2>&1; then
    unzip -p "$dir/$name.download" "$name" > "$dir/$name"
  else
    mv "$dir/$name.download" "$dir/$name"
  fi
  rm -f "$dir/$name.download"
done < "$dir/list"

# Every file must match its checksum before anything is uploaded.
cd "$dir"
for sum in *.sha256; do
  sha256sum -c "$sum"
done
for f in *; do
  case "$f" in list) continue ;; esac
  case "$f" in *.sha256) ;; *) test -f "$f.sha256" ;; esac
done

for f in *; do
  case "$f" in list) continue ;; esac
  gh api -X POST "https://uploads.github.com/repos/$repo/releases/$id/assets?name=$f" \
    -H "Content-Type: application/octet-stream" --input "$f" --jq .browser_download_url
done
