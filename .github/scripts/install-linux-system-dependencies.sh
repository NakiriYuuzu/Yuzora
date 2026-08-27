#!/usr/bin/env bash
set -euo pipefail

# GitHub's Ubuntu images may select azure.archive.ubuntu.com through either a
# source file or apt-mirrors.txt. It stalled for the full database-job timeout
# in CI run 32091917804, so use the canonical Ubuntu archive instead.
apt_sources=()
for source in /etc/apt/sources.list /etc/apt/apt-mirrors.txt; do
  [ -f "$source" ] && apt_sources+=("$source")
done
while IFS= read -r -d '' source; do
  apt_sources+=("$source")
done < <(find /etc/apt/sources.list.d -type f \( -name '*.list' -o -name '*.sources' \) -print0 2>/dev/null)

for source in "${apt_sources[@]}"; do
  sed -Ei 's#https?://azure\.archive\.ubuntu\.com/ubuntu/?#https://archive.ubuntu.com/ubuntu/#g' "$source"

  case "$source" in
    *.list|*/sources.list)
      sed -Ei '/^[[:space:]]*deb(-src)?([[:space:]]+\[[^]]+\])?[[:space:]]+https:\/\/archive\.ubuntu\.com\/ubuntu\/[[:space:]]+[^[:space:]]*-security([[:space:]]|$)/ s#https://archive\.ubuntu\.com/ubuntu/#https://security.ubuntu.com/ubuntu/#' "$source"
      ;;
    *.sources)
      perl -0pi -e 's{(URIs:\s*)https://archive\.ubuntu\.com/ubuntu/(\s*\nSuites:\s*[^\n]*\b\S+-security\b)}{$1https://security.ubuntu.com/ubuntu/$2}g' "$source"
      ;;
  esac
done

apt_options=(
  -o Acquire::Retries=3
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
  -o Acquire::http::Pipeline-Depth=0
)

timeout --foreground 8m apt-get "${apt_options[@]}" update
timeout --foreground 12m env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" install -y --no-install-recommends \
  build-essential \
  curl \
  file \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  libxdo-dev \
  patchelf \
  wget
