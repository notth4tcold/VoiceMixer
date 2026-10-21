#!/bin/sh
# Builds the Arch package from the current checkout, inside an
# archlinux:base-devel container. Used by CI and by the release workflow.
# usage: packaging/arch/ci-build.sh <version> <output dir>
set -eu

ver="$1"
out="$2"
src="$(pwd)"

git config --global --add safe.directory "$src"
id builder >/dev/null 2>&1 || useradd -m builder
rm -rf /build
mkdir -p /build "$out"
cp packaging/arch/PKGBUILD /build/
git archive --prefix="VoiceMixer-$ver/" -o "/build/voicemixer-$ver.tar.gz" HEAD
sed -i "s/^pkgver=.*/pkgver=$ver/; s|^source=.*|source=(\"voicemixer-$ver.tar.gz\")|" /build/PKGBUILD
chown -R builder /build

status=0
(cd /build && sudo -u builder makepkg --noconfirm) > /build/makepkg.log 2>&1 || status=$?
cat /build/makepkg.log
if [ "$status" -ne 0 ]; then
    # Logs need admin rights to download; a multi-line annotation is public.
    msg=$( { grep -E -A4 'error|ERROR' /build/makepkg.log | head -40; tail -n 25 /build/makepkg.log; } \
        | sed -e 's/%/%25/g' | sed -e ':a;N;$!ba;s/\n/%0A/g')
    echo "::error title=makepkg failed::$msg"
    exit "$status"
fi
cp /build/voicemixer-*.pkg.tar.zst "$out/voicemixer-x86_64.pkg.tar.zst"
