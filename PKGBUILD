pkgname=bookos-store
pkgver=0.4.0
pkgrel=1
pkgdesc="Tienda de apps para el ecosistema BookOS — instalar, actualizar y desinstalar"
arch=('x86_64')
url="https://bookos.es/"
license=('MIT')
depends=('webkit2gtk-4.1' 'gtk3' 'libsoup3' 'librsvg' 'curl' 'polkit' 'xdg-utils')
makedepends=('rust' 'cargo' 'pkgconf' 'base-devel' 'imagemagick')
source=()
options=('!strip' '!debug')

build() {
  cd "$startdir/src-tauri"
  cargo build --release --locked
}

package() {
  install -Dm755 "$startdir/src-tauri/target/release/bookos-store" \
    "$pkgdir/usr/bin/bookos-store"

  install -Dm644 /dev/stdin "$pkgdir/usr/share/applications/bookos-store.desktop" <<EOF
[Desktop Entry]
Name=Bookos Store
GenericName=Tienda de apps
Comment=Instala y gestiona apps del ecosistema BookOS
Exec=bookos-store
Icon=bookos-store
Type=Application
Categories=Utility;System;PackageManager;
StartupNotify=true
StartupWMClass=Bookos Store
Keywords=apps;store;install;tienda;
EOF

  if [ -f "$startdir/src-tauri/icons/icon.png" ]; then
    for sz in 16 22 24 32 48 64 96 128 256 512; do
      install -d "$pkgdir/usr/share/icons/hicolor/${sz}x${sz}/apps"
      magick "$startdir/src-tauri/icons/icon.png" \
        -resize ${sz}x${sz} \
        "$pkgdir/usr/share/icons/hicolor/${sz}x${sz}/apps/bookos-store.png" \
        2>/dev/null || true
    done
  fi

  [ -f "$startdir/src-tauri/icons/icon.svg" ] && install -Dm644 "$startdir/src-tauri/icons/icon.svg" \
    "$pkgdir/usr/share/icons/hicolor/scalable/apps/bookos-store.svg"
}
