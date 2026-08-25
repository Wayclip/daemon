ICON_SRC="branding"
DEST="/usr/share/icons/hicolor"

install -Dm644 "$ICON_SRC/svgs/wayclip.svg" "$DEST/scalable/apps/wayclip.svg"
install -Dm644 "$ICON_SRC/pngs/wayclip-32x32.png" "$DEST/32x32/apps/wayclip.png"
install -Dm644 "$ICON_SRC/pngs/wayclip-64x64.png" "$DEST/64x64/apps/wayclip.png"
install -Dm644 "$ICON_SRC/pngs/wayclip-128x128.png" "$DEST/128x128/apps/wayclip.png"
install -Dm644 "$ICON_SRC/pngs/wayclip-256x256.png" "$DEST/256x256/apps/wayclip.png"

gtk-update-icon-cache -f -t "$DEST" 2>/dev/null || true
