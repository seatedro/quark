#!/usr/bin/env bash
# Build an .rpm of the hello_ui example from an already built binary.
# cargo-packager has no rpm format, so this drives rpmbuild directly.
#
#   packaging/linux/build-rpm.sh <binary> <version> <out-dir>
#
# Needs rpmbuild (Debian and Ubuntu: `apt-get install rpm`) and the icons
# from packaging/icons.py. Installs the binary as /usr/bin/hello_ui, like
# the deb, with a desktop entry and a 256 px icon.
set -euo pipefail

if [ $# -ne 3 ]; then
  echo "usage: $0 <binary> <version> <out-dir>" >&2
  exit 2
fi
binary=$(realpath "$1")
# rpm versions cannot contain '-'; a prerelease 1.2.0-beta.1 becomes
# version 1.2.0 with release 0.beta.1, which sorts before release 1.
version=${2%%-*}
if [ "$version" = "$2" ]; then release=1; else release="0.${2#*-}"; fi
out=$(realpath -m "$3")
here=$(cd "$(dirname "$0")/.." && pwd)
icon="$here/icons/256x256.png"
[ -f "$icon" ] || { echo "missing $icon: run packaging/icons.py" >&2; exit 1; }

top=$(mktemp -d)
trap 'rm -rf "$top"' EXIT
mkdir -p "$top/SOURCES" "$out"
cp "$binary" "$top/SOURCES/hello_ui"
cp "$icon" "$top/SOURCES/hello_ui.png"
cat >"$top/SOURCES/hello_ui.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Quark Hello
Exec=hello_ui
Icon=hello_ui
Terminal=false
Categories=Development;
DESKTOP

cat >"$top/quark-hello.spec" <<SPEC
Name:           quark-hello
Version:        $version
Release:        $release
Summary:        The Quark hello_ui example
License:        MIT
URL:            https://github.com/seatedro/quark
# The binary is prebuilt; rpmbuild must not strip or rewrite it.
%global debug_package %{nil}
%global __strip /bin/true

%description
The Quark hello_ui example, a GPU rendered native window.

%install
install -Dm0755 %{_sourcedir}/hello_ui %{buildroot}/usr/bin/hello_ui
install -Dm0644 %{_sourcedir}/hello_ui.desktop %{buildroot}/usr/share/applications/hello_ui.desktop
install -Dm0644 %{_sourcedir}/hello_ui.png %{buildroot}/usr/share/icons/hicolor/256x256/apps/hello_ui.png

%files
/usr/bin/hello_ui
/usr/share/applications/hello_ui.desktop
/usr/share/icons/hicolor/256x256/apps/hello_ui.png
SPEC

rpmbuild -bb --define "_topdir $top" "$top/quark-hello.spec"
find "$top/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
ls "$out"/*.rpm
