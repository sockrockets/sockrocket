#!/usr/bin/env bash
# Install native GUI build dependencies on the Ubuntu CI runners.
set -euo pipefail
source /etc/os-release
case "$(dpkg --print-architecture)" in
  amd64) archive=ubuntu; primary=https://archive.ubuntu.com ;;
  arm64) archive=ubuntu-ports; primary=https://ports.ubuntu.com ;;
  *) echo "Unsupported Ubuntu runner architecture" >&2; exit 1 ;;
esac
packages=(libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-dev
          libvulkan-dev libgl1-mesa-dev libasound2-dev libfontconfig1-dev
          libfreetype6-dev pkg-config clang "$@")
sources=$(mktemp --suffix=.list)
trap 'rm -f "$sources"' EXIT
chmod 644 "$sources"
options=(-o "Dir::Etc::sourcelist=$sources" -o Dir::Etc::sourceparts=-
         -o APT::Update::Error-Mode=any -o Acquire::https::Timeout=20)
for mirror in "$primary" https://mirrors.mit.edu; do
  root="$mirror/$archive"
  if ! curl -fsSL --max-time 8 -o /dev/null "$root/dists/$VERSION_CODENAME/InRelease"; then
    continue
  fi
  for suite in "$VERSION_CODENAME" "$VERSION_CODENAME-updates" "$VERSION_CODENAME-security"; do
    echo "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] $root $suite main universe"
  done >"$sources"
  if sudo apt-get "${options[@]}" update && sudo apt-get "${options[@]}" install -y "${packages[@]}"; then
    exit 0
  fi
done
echo "Unable to install Ubuntu build dependencies from either signed archive" >&2
exit 1
