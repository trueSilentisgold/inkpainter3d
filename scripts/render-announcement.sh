#!/usr/bin/env bash
# Renders a release announcement's image for Discord:
#   scripts/render-announcement.sh v0.1.0
# reads .github/announcements/<tag>/image.svg and writes image.png next to it, with the
# app's fonts. Needs resvg (`cargo install resvg --locked`).
set -euo pipefail

tag="${1:?usage: scripts/render-announcement.sh <tag>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/.github/announcements/$tag"

resvg --skip-system-fonts --use-fonts-dir "$root/assets/fonts" \
  --resources-dir "$dir" "$dir/image.svg" "$dir/image.png"
echo "$dir/image.png"
