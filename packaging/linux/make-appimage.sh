#!/bin/sh
# Make the AppImage from WeekNumber's static Linux executable.
#
#   packaging/linux/make-appimage.sh <executable> <output.AppImage>
#
# The executable needs no runtime, so it is the AppImage's ELF part as is:
# it gets the AppImage type 2 magic bytes ("AI" 2 at offset 8, a field
# Linux ignores) and a small SquashFS with the menu entry and the icons,
# written by the program itself (--appimage-extract), is appended where
# AppImage tools look for it -- the end of the ELF section headers. No FUSE
# and no downloaded runtime are involved. Needs mksquashfs (squashfs-tools).
set -eu

exe=$1
out=$2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cp "$exe" "$work/elf"
chmod 755 "$work/elf"

# The payload must start right after the ELF image: its section header
# table has to be the last thing in the file.
u16() { od -An -tu2 -j "$1" -N2 "$work/elf" | tr -d ' '; }
u64() { od -An -tu8 -j "$1" -N8 "$work/elf" | tr -d ' '; }
end=$(( $(u64 40) + $(u16 58) * $(u16 60) ))
size=$(stat -c %s "$work/elf")
if [ "$end" -ne "$size" ]; then
  echo "the ELF section headers do not end the file ($end != $size)" >&2
  exit 1
fi

printf 'AI\002' | dd of="$work/elf" bs=1 seek=8 conv=notrunc status=none

# The AppDir, from the program. In the payload AppRun only hands over to
# the AppImage file (tools that run AppRun set $APPIMAGE); the program
# itself is the AppImage's ELF part.
(cd "$work" && ./elf --appimage-extract >/dev/null)
cat > "$work/squashfs-root/AppRun" <<'SH'
#!/bin/sh
# WeekNumber's AppImage is the program itself: run the AppImage file.
exec "${APPIMAGE:?run the WeekNumber AppImage file itself}" "$@"
SH
chmod 755 "$work/squashfs-root/AppRun"

mksquashfs "$work/squashfs-root" "$work/payload.sqfs" \
  -comp gzip -root-owned -no-xattrs -noappend -quiet \
  -mkfs-time 0 -all-time 0 >/dev/null

cat "$work/elf" "$work/payload.sqfs" > "$out"
chmod 755 "$out"

# The result reports its own payload offset and lists the AppDir.
test "$("$out" --appimage-offset)" = "$size"
unsquashfs -o "$size" -l "$out" | sed -n 's|^squashfs-root/||p'
