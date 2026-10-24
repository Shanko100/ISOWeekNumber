#!/usr/bin/env bash
# ============================================================
#  WeekNumber macOS build - the macOS counterpart of build_exe.bat.
#
#  Usage:  ./build_app.sh [--venv]
#    (default)  dist/WeekNumber.app and dist/WeekNumber.dmg with the
#               current Python
#    --venv     build inside an isolated .venv-build-mac with the
#               hash-pinned release dependencies (requirements-release.lock)
#
#  Python: 3.15, the version requirements-release.lock is made for.
#
#  Architecture: the app is built for the Python that runs this
#  script -- arm64 Python gives a native Apple silicon app, x86_64
#  Python an Intel app (on Apple silicon it then runs under Rosetta,
#  so the script warns). Use the python.org universal2 installer or
#  an arm64 Homebrew Python on Apple silicon.
#
#  The bundle is a menu-bar accessory (LSUIElement): no Dock icon,
#  like the Windows tray app. It is ad-hoc signed so it runs on the
#  build machine; sign with a Developer ID and notarize before
#  publishing.
# ============================================================
set -euo pipefail
cd "$(dirname "$0")"

USEVENV=0
for arg in "$@"; do
    case "$arg" in
        --venv) USEVENV=1 ;;
        *) echo "Unknown option: $arg"; exit 2 ;;
    esac
done

echo "============================================================"
echo " WeekNumber .app build"
echo "============================================================"

echo
echo "[1/7] Checking project files..."
REQUIRED=(weeknumber.py weeknumber_core.py weeknumber_tray.py weeknumber_menubar.py
          weeknumber_calendar.py weeknumber_clipboard.py weeknumber_timezone.py
          weeknumber_timezone_dialog.py weeknumber_holiday_dialog.py weeknumber_about.py
          weeknumber_license_dialog.py weeknumber_color_dialog.py
          weeknumber_widgets.py weeknumber_dialog.py weeknumber_map.py weeknumber_tzd_rules.py
          weeknumber_tzd_net.py weeknumber_tzd_relay.py weeknumber_tzd_board.py weeknumber_tzd_lobby.py
          weeknumber_tzd_dialog.py weeknumber_tzd_chat.py weeknumber_tzd_chat_view.py
          weeknumber_tzd_fx.py weeknumber_tzd_sound.py weeknumber_platform/__init__.py
          weeknumber_platform/_common.py weeknumber_platform/_macos.py
          weeknumber_platform/_cocoa.py weeknumber_platform/_macos_tz.py
          collect_licenses.py holiday_data.jsonl
          timezone_data.json tzd_data.json app.ico app.icns)
for file in "${REQUIRED[@]}"; do
    if [[ ! -f "$file" ]]; then
        echo "      ERROR: $file not found."
        exit 1
    fi
done
echo "      OK"

echo
echo "[2/7] Build environment..."
PY="${PYTHON:-python3}"
if [[ "$USEVENV" == "1" ]]; then
    if [[ ! -x .venv-build-mac/bin/python ]]; then
        echo "      Creating .venv-build-mac..."
        "$PY" -m venv .venv-build-mac
        .venv-build-mac/bin/python -m pip install --quiet --require-hashes \
            --only-binary :all: -r ../../requirements-release.lock
    fi
    PY=.venv-build-mac/bin/python
    echo "      OK - using .venv-build-mac"
else
    echo "      Using $("$PY" --version) as-is."
fi
"$PY" -c "import sys; sys.exit(0 if sys.version_info[:2] == (3, 15) else 1)" || {
    echo "      ERROR: $("$PY" --version) found; WeekNumber builds with Python 3.15,"
    echo "      the version requirements-release.lock pins every dependency for."
    exit 1
}
"$PY" -c "import PIL, PyInstaller" 2>/dev/null || {
    echo "      ERROR: Pillow and PyInstaller are required."
    exit 1
}
"$PY" -c "import tkinter" 2>/dev/null || {
    echo "      ERROR: this Python has no tkinter (Homebrew: brew install python-tk)."
    exit 1
}
ARCH=$("$PY" -c "import platform; print(platform.machine())")
APPLE_SILICON=$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)
case "$ARCH" in
    arm64) echo "      Architecture: arm64 (native on Apple silicon)" ;;
    x86_64)
        if [[ "$APPLE_SILICON" == "1" ]]; then
            echo "      WARNING: this Python runs as x86_64 under Rosetta, so the app"
            echo "      will be an Intel build. Use an arm64 Python for a native one."
        else
            echo "      Architecture: x86_64 (Intel Mac)"
        fi ;;
    *) echo "      ERROR: unsupported architecture $ARCH."; exit 1 ;;
esac

VERSION=$("$PY" -c "import re,pathlib; print(re.search(r'APP_VERSION = \"([^\"]+)\"', pathlib.Path('weeknumber_core.py').read_text()).group(1))")
echo "      Version $VERSION"

echo
echo "[3/7] Building WeekNumber.app..."
#   --optimize 2   same as the Windows build: no asserts/docstrings needed
#   excludes       the Windows backend and tray never load on macOS; nor
#                  does PyObjC (AppKit goes through _cocoa), even if a
#                  library mentions it
#   absolute paths: --specpath makes PyInstaller resolve relative ones in build/
#   Licenses.txt   every license the bundle carries; About > Legal notices shows it
#   _cffi_backend  PyNaCl's libsodium core imports it from C, where PyInstaller
#                  cannot see it, and the nacl hook does not add it
#   PIL_EXCLUDES   Pillow's image-file readers and writers and their native
#                  codecs (AVIF, WebP, colour management -- several MB): the
#                  app draws its images and opens one file, app.ico, with the
#                  ICO reader (PNG and BMP frames), which stays. Pillow skips
#                  readers it cannot import. Same list as build_exe.bat.
#   hooks          PyInstaller's runtime hook for inspect imports inspect and
#                  zipfile at every launch (about 1 MB that stays loaded) to
#                  fix up source-file lookups of frozen modules; the app never
#                  reads its sources. An empty hook of the same name in
#                  build/hooks takes its place.
PIL_EXCLUDES=()
for module in AvifImagePlugin BlpImagePlugin BufrStubImagePlugin CurImagePlugin \
        DcxImagePlugin DdsImagePlugin EpsImagePlugin FitsImagePlugin FliImagePlugin \
        FpxImagePlugin FtexImagePlugin GbrImagePlugin GifImagePlugin \
        GribStubImagePlugin Hdf5StubImagePlugin IcnsImagePlugin ImImagePlugin \
        ImtImagePlugin IptcImagePlugin Jpeg2KImagePlugin JpegImagePlugin JpegPresets \
        McIdasImagePlugin MicImagePlugin MpegImagePlugin MpoImagePlugin MspImagePlugin \
        PalmImagePlugin PcdImagePlugin PcxImagePlugin PdfImagePlugin PdfParser \
        PixarImagePlugin PpmImagePlugin PsdImagePlugin QoiImagePlugin SgiImagePlugin \
        SpiderImagePlugin SunImagePlugin TgaImagePlugin TiffImagePlugin \
        WebPImagePlugin WmfImagePlugin XVThumbImagePlugin XbmImagePlugin \
        XpmImagePlugin ImageCms ImageDraw2 ImagePath ImageShow ImageWin features \
        _avif _webp _imagingcms _imagingmath _imagingmorph; do
    PIL_EXCLUDES+=(--exclude-module "PIL.$module")
done
HERE="$(pwd)"
mkdir -p build/hooks/rthooks
echo '{"inspect": ["pyi_rth_inspect.py"]}' > build/hooks/rthooks.dat
echo '# Generated by build_app.sh: no inspect fix-up at startup.' \
    > build/hooks/rthooks/pyi_rth_inspect.py
"$PY" collect_licenses.py "$HERE/build/Licenses.txt"
"$PY" -m PyInstaller --clean --noconfirm --windowed --name WeekNumber \
    --icon "$HERE/app.icns" \
    --osx-bundle-identifier io.github.shanko100.weeknumber \
    --add-data "$HERE/holiday_data.jsonl:." \
    --add-data "$HERE/timezone_data.json:." \
    --add-data "$HERE/tzd_data.json:." \
    --add-data "$HERE/app.ico:." \
    --add-data "$HERE/build/Licenses.txt:." \
    --optimize 2 --specpath build \
    --additional-hooks-dir "$HERE/build/hooks" \
    --hidden-import _cffi_backend \
    --exclude-module weeknumber_platform._windows \
    --exclude-module weeknumber_platform._windows_tz \
    --exclude-module weeknumber_platform._wintray \
    --exclude-module objc --exclude-module AppKit --exclude-module Foundation \
    --exclude-module CoreFoundation \
    --exclude-module PIL.ImageQt --exclude-module PyQt5 --exclude-module PyQt6 \
    --exclude-module PySide2 --exclude-module PySide6 \
    --exclude-module IPython --exclude-module matplotlib \
    --exclude-module numpy --exclude-module pandas \
    "${PIL_EXCLUDES[@]}" \
    weeknumber.py
APP=dist/WeekNumber.app
echo "      OK"

echo
echo "[4/7] Writing Info.plist keys..."
"$PY" - "$APP/Contents/Info.plist" "$VERSION" <<'PYEOF'
import plistlib, sys
path, version = sys.argv[1], sys.argv[2]
with open(path, "rb") as handle:
    info = plistlib.load(handle)
info.update({
    "LSUIElement": True,                        # menu-bar extra, no Dock icon
    "CFBundleShortVersionString": version,
    "CFBundleVersion": version,
    "LSMinimumSystemVersion": "12.0",
    "NSHumanReadableCopyright": "Copyright (c) 2026 Shanko100",
    # Shown by macOS the first time the hidden game looks for lobbies.
    "NSLocalNetworkUsageDescription":
        "WeekNumber looks for game lobbies on your local network and lets "
        "colleagues join a game you host.",
})
with open(path, "wb") as handle:
    plistlib.dump(info, handle)
PYEOF
echo "      OK"

echo
echo "[5/7] Ad-hoc signing..."
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
echo "      OK (ad-hoc; use a Developer ID and notarize for distribution)"

echo
echo "[6/7] Verifying bundle..."
for resource in holiday_data.jsonl timezone_data.json tzd_data.json Licenses.txt; do
    if ! find "$APP/Contents" -name "$resource" | grep -q .; then
        echo "      ERROR: Required packaged resource $resource is missing."
        exit 1
    fi
done
# Chat's encryption: PyNaCl's libsodium core and cffi's backend are native;
# Pillow's core, fonts and Tk bridge draw every window and the menu bar.
for library in "_sodium*.so" "_cffi_backend*.so" "_imaging.*so" "_imagingft*.so" \
               "_imagingtk*.so"; do
    if ! find "$APP/Contents" -name "$library" | grep -q .; then
        echo "      ERROR: Required native library $library is missing."
        exit 1
    fi
done
# Window modules load on first use, so a missing one would only show on a
# click: check the packaged archive like build_exe.bat does. If this
# PyInstaller's archive viewer cannot read the bundle, say so and go on.
ARCHIVE_LIST=build/archive_contents.txt
if "$PY" -m PyInstaller.utils.cliutils.archive_viewer -l -r "$APP/Contents/MacOS/WeekNumber" \
        > "$ARCHIVE_LIST" 2>/dev/null; then
    for module in weeknumber_calendar weeknumber_timezone_dialog weeknumber_holiday_dialog \
                  weeknumber_about weeknumber_license_dialog weeknumber_color_dialog \
                  weeknumber_tzd_lobby weeknumber_tzd_dialog weeknumber_tzd_chat \
                  weeknumber_tzd_sound weeknumber_platform._macos weeknumber_menubar \
                  PIL.IcoImagePlugin PIL.PngImagePlugin PIL.BmpImagePlugin; do
        if ! grep -q "$module" "$ARCHIVE_LIST"; then
            echo "      ERROR: Required packaged module $module is missing."
            exit 1
        fi
    done
else
    echo "      NOTE: could not list the packaged modules; skipped that check."
fi
echo "      OK - $(du -sh "$APP" | cut -f1)  $(cd dist && pwd)/WeekNumber.app"
# What the size is made of: the ten largest files in the bundle.
echo "      Largest files [KB]:"
# (awk reads to the end, so no stage dies of SIGPIPE under pipefail)
find "$APP/Contents" -type f -exec du -k {} + | sort -rn \
    | awk -v prefix="$APP/Contents/" 'NR <= 10 { sub(prefix, "", $2); printf "        %6d  %s\n", $1, $2 }'

echo
echo "[7/7] Building WeekNumber.dmg..."
# The app, a shortcut to drag it onto, and the licenses beside them.
# ULMO (LZMA) is the smallest compressed image format; macOS 10.15 and later
# open it, and the app needs 12.
STAGE=build/dmg
rm -rf "$STAGE" dist/WeekNumber.dmg
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/WeekNumber.app"
ln -s /Applications "$STAGE/Applications"
cp build/Licenses.txt "$STAGE/Licenses.txt"
hdiutil create -quiet -volname "WeekNumber $VERSION" -srcfolder "$STAGE" \
    -fs HFS+ -format ULMO dist/WeekNumber.dmg
hdiutil verify -quiet dist/WeekNumber.dmg
rm -rf "$STAGE"
echo "      OK - $(du -h dist/WeekNumber.dmg | cut -f1)  $(cd dist && pwd)/WeekNumber.dmg"

echo
echo "============================================================"
echo " SUCCESS: Build completed ($ARCH)."
echo "============================================================"
