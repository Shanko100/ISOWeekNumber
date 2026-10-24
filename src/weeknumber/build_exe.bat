@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

:: ============================================================
::  WeekNumber build - the only build file in this project.
::
::  Usage:  build_exe.bat [--onedir] [--venv] [--nopause]
::    (default)  one-file dist\WeekNumber.exe
::    --onedir   folder build dist\WeekNumber\ - starts faster at
::               logon (no per-launch %TEMP% extraction / AV rescan)
::    --venv     build inside an isolated .venv-build with the
::               hash-pinned requirements-release.lock: smallest,
::               reproducible exe regardless of what is installed
::    --nopause  no interactive pause; returns exit code 0/1
::
::  The PyInstaller spec and the VERSIONINFO resource are
::  generated into build\ on every run; the version number is
::  read from APP_VERSION in weeknumber_core.py.
::
::  Python: 3.15, the version requirements-release.lock is made
::  for. Architecture: the exe is built for the Python that runs
::  this script - x64 Python gives an x64 exe (also runs on
::  Windows 11 on ARM, emulated), ARM64 Python a native ARM64
::  exe; both have every feature, chat included. 32-bit Python
::  is refused.
:: ============================================================

set "BUILD_OK=0"
set "PY="
set "MODE=--onefile"
set "OUTEXE=dist\WeekNumber.exe"
set "NOPAUSE=0"
set "USEVENV=0"

for %%A in (%*) do (
    if /i "%%~A"=="--nopause" set "NOPAUSE=1"
    if /i "%%~A"=="--venv"    set "USEVENV=1"
    if /i "%%~A"=="--onedir" (
        set "MODE="
        set "OUTEXE=dist\WeekNumber\WeekNumber.exe"
    )
)

echo ============================================================
echo  WeekNumber EXE build
echo ============================================================
echo.

:: ------------------------------------------------------------
:: [1/9] Check required files
:: ------------------------------------------------------------
echo [1/9] Checking project files...
echo       Folder: !CD!

for %%F in (weeknumber.py weeknumber_core.py weeknumber_tray.py weeknumber_calendar.py weeknumber_clipboard.py weeknumber_timezone.py weeknumber_timezone_dialog.py weeknumber_holiday_dialog.py weeknumber_about.py weeknumber_license_dialog.py weeknumber_color_dialog.py weeknumber_widgets.py weeknumber_dialog.py weeknumber_map.py weeknumber_tzd_rules.py weeknumber_tzd_net.py weeknumber_tzd_relay.py weeknumber_tzd_board.py weeknumber_tzd_lobby.py weeknumber_tzd_dialog.py weeknumber_tzd_chat.py weeknumber_tzd_chat_view.py weeknumber_tzd_fx.py weeknumber_tzd_sound.py weeknumber_platform\__init__.py weeknumber_platform\_common.py weeknumber_platform\_windows.py weeknumber_platform\_windows_tz.py weeknumber_platform\_wintray.py generate_holiday_data.py collect_licenses.py country_names.json holiday_data.jsonl timezone_data.json tzd_data.json app.ico) do (
    if not exist "%%F" (
        echo       ERROR: %%F not found.
        goto :DONE
    )
)

echo       OK

:: ------------------------------------------------------------
:: [2/9] Configure required icon resource
:: ------------------------------------------------------------
echo.
echo [2/9] Configuring app.ico...

set "ICON_ARG=--icon "!CD!\app.ico" --add-data "!CD!\app.ico;.""
echo       OK - app.ico is used for the EXE and all application windows.

:: ------------------------------------------------------------
:: [3/9] Detect Python
::   `where python` alone is not enough: the Microsoft Store
::   alias is a python.exe on PATH that only prints an install
::   hint. Validate by executing, fall back to the py launcher.
:: ------------------------------------------------------------
echo.
echo [3/9] Detecting Python...

python -c "import sys" >nul 2>nul
if not errorlevel 1 (
    set "PY=python"
) else (
    py -3 -c "import sys" >nul 2>nul
    if not errorlevel 1 set "PY=py -3"
)

if not defined PY (
    echo       ERROR: No working Python found. Checked: python, py -3.
    echo       Note: a PATH entry alone can be the Microsoft Store alias,
    echo       which is not a real interpreter.
    goto :DONE
)

for /f "delims=" %%V in ('!PY! --version 2^>nul') do set "PY_VERSION=%%V"

:: The process architecture: AMD64 or ARM64 (platform.machine),
:: 32 for a 32-bit interpreter whatever the OS runs on.
set "ARCH="
for /f "delims=" %%A in ('!PY! -c "import platform, sys; print(platform.machine().upper() if sys.maxsize.bit_length() == 63 else 32)" 2^>nul') do set "ARCH=%%A"
if /i "!ARCH!"=="AMD64" set "ARCH=x64"
if /i "!ARCH!"=="X86_64" set "ARCH=x64"
if /i "!ARCH!"=="ARM64" set "ARCH=ARM64"
if /i "!ARCH!"=="AARCH64" set "ARCH=ARM64"
if not "!ARCH!"=="x64" if not "!ARCH!"=="ARM64" (
    echo       ERROR: !PY_VERSION! is a 32-bit or unknown [!ARCH!] interpreter. WeekNumber builds
    echo       with 64-bit Python only: install the x64 or ARM64 Python
    echo       from python.org and run this script with it.
    goto :DONE
)

!PY! -c "import sys; sys.exit(0 if sys.version_info[:2] == (3, 15) else 1)" >nul 2>nul
if errorlevel 1 (
    echo       ERROR: !PY_VERSION! found; WeekNumber builds with Python 3.15, the
    echo       version requirements-release.lock pins every dependency for.
    goto :DONE
)

!PY! -c "import tkinter" >nul 2>nul
if errorlevel 1 (
    echo       ERROR: this Python has no tkinter. Reinstall it from python.org
    echo       with "tcl/tk and IDLE" selected.
    goto :DONE
)
echo       OK - !PY_VERSION! [!ARCH!] via "!PY!"

:: ------------------------------------------------------------
:: [4/9] Isolated build venv (--venv)
:: ------------------------------------------------------------
echo.
echo [4/9] Build environment...

if "%USEVENV%"=="0" (
    echo       Using the detected Python environment as-is.
    echo       Hint: --venv builds in a clean pinned venv - smallest,
    echo       reproducible exe independent of global site-packages.
    goto :ENV_DONE
)

if not exist ".venv-build\Scripts\python.exe" (
    echo       Creating .venv-build...
    !PY! -m venv .venv-build
    if errorlevel 1 (
        echo       ERROR: venv creation failed.
        goto :DONE
    )
    echo       Installing pinned build dependencies...
    .venv-build\Scripts\python.exe -m pip install --quiet --require-hashes --only-binary :all: -r ..\..\requirements-release.lock
    if errorlevel 1 (
        echo       ERROR: dependency install failed. Delete .venv-build and retry.
        goto :DONE
    )
)
set "PY=.venv-build\Scripts\python.exe"
echo       OK - using .venv-build

:ENV_DONE

:: ------------------------------------------------------------
:: [5/9] Generate the bundled offline nationwide holiday data
:: ------------------------------------------------------------
echo.
echo [5/9] Generating nationwide public-holiday data...

!PY! -c "import holidays, importlib.metadata, sys; sys.exit(0 if importlib.metadata.version('holidays') == '0.106' else 1)" >nul 2>nul
if errorlevel 1 (
    echo       ERROR: The pinned build dependency holidays==0.106 is unavailable.
    echo.
    echo       Fix, either:
    echo         build_exe.bat --venv
    echo       or:
    echo         !PY! -m pip install "holidays==0.106"
    goto :DONE
)

!PY! generate_holiday_data.py
if errorlevel 1 (
    echo       ERROR: Holiday-data generation failed.
    goto :DONE
)
echo       OK

:: ------------------------------------------------------------
:: [6/9] Check PyInstaller (needs >= 6.6 for --optimize)
:: ------------------------------------------------------------
echo.
echo [6/9] Checking PyInstaller...

!PY! -c "import PyInstaller, sys; v = tuple(int(x) for x in PyInstaller.__version__.split('.')[:2]); sys.exit(0 if v >= (6, 6) else 1)" >nul 2>nul
if errorlevel 1 (
    echo       ERROR: PyInstaller 6.6+ not available in this environment.
    echo.
    echo       Fix, either:
    echo         build_exe.bat --venv
    echo       or:
    echo         !PY! -m pip install --require-hashes --only-binary :all: -r ..\..\requirements-release.lock
    goto :DONE
)

for /f "delims=" %%V in ('!PY! -m PyInstaller --version 2^>nul') do set "PYI_VERSION=%%V"
echo       OK - PyInstaller !PYI_VERSION!

:: ------------------------------------------------------------
:: [7/9] Clean old build, generate the VERSIONINFO resource
:: ------------------------------------------------------------
echo.
echo [7/9] Cleaning and preparing build\ ...

if exist "build" rmdir /s /q "build"
if exist "dist"  rmdir /s /q "dist"

if exist "%OUTEXE%" (
    echo       ERROR: %OUTEXE% is locked - a WeekNumber started from
    echo       dist\ is still running. Exit it via the tray menu, then
    echo       rebuild.
    goto :DONE
)

mkdir "build"

:: Version = APP_VERSION in weeknumber_core.py (single source of truth).
:: Classic findstr idiom: the line's two quotes make the version token 2.
set "APPVER="
for /f tokens^=2^ delims^=^" %%v in ('findstr /r /c:"^APP_VERSION" weeknumber_core.py') do set "APPVER=%%v"

if not defined APPVER (
    echo       ERROR: could not read APP_VERSION from weeknumber_core.py.
    goto :DONE
)
set "COPYRIGHT="
for /f tokens^=2^ delims^=^" %%v in ('findstr /r /c:"^APP_COPYRIGHT" weeknumber_core.py') do set "COPYRIGHT=%%v"
if not defined COPYRIGHT (
    echo       ERROR: could not read APP_COPYRIGHT from weeknumber_core.py.
    goto :DONE
)
set "QUADVER=%APPVER%.0.0"
set "QUADVER_TUPLE=%APPVER:.=, %, 0, 0"
echo       App version: %APPVER%  (resource %QUADVER%)

:: Absolute: with --specpath, PyInstaller resolves relative file
:: arguments against the spec directory, not the CWD.
set "VERFILE=!CD!\build\version_info.txt"
> "!VERFILE!" echo # Generated by build_exe.bat - do not edit; source is APP_VERSION.
>>"!VERFILE!" echo VSVersionInfo(
>>"!VERFILE!" echo   ffi=FixedFileInfo(
>>"!VERFILE!" echo     filevers=(%QUADVER_TUPLE%),
>>"!VERFILE!" echo     prodvers=(%QUADVER_TUPLE%),
>>"!VERFILE!" echo     mask=0x3F, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0),
>>"!VERFILE!" echo   ),
>>"!VERFILE!" echo   kids=[
>>"!VERFILE!" echo     StringFileInfo([
>>"!VERFILE!" echo       StringTable(
>>"!VERFILE!" echo         "040904B0",
>>"!VERFILE!" echo         [
>>"!VERFILE!" echo           StringStruct("FileDescription", "WeekNumber - ISO week number in the system tray"),
>>"!VERFILE!" echo           StringStruct("FileVersion", "%QUADVER%"),
>>"!VERFILE!" echo           StringStruct("InternalName", "WeekNumber"),
>>"!VERFILE!" echo           StringStruct("LegalCopyright", "%COPYRIGHT%"),
>>"!VERFILE!" echo           StringStruct("OriginalFilename", "WeekNumber.exe"),
>>"!VERFILE!" echo           StringStruct("ProductName", "WeekNumber"),
>>"!VERFILE!" echo           StringStruct("ProductVersion", "%QUADVER%"),
>>"!VERFILE!" echo         ],
>>"!VERFILE!" echo       )
>>"!VERFILE!" echo     ]),
>>"!VERFILE!" echo     VarFileInfo([VarStruct("Translation", [1033, 1200])]),
>>"!VERFILE!" echo   ],
>>"!VERFILE!" echo )

echo       OK

:: ------------------------------------------------------------
:: [8/9] Build
::   --optimize 2   strip asserts+docstrings from bytecode; the
::                  app uses neither at runtime (verified)
::   --noupx        UPX would shrink the exe but raises AV
::                  false-positive rates; a flagged exe is lost
::                  functionality
::   --specpath     PyInstaller always writes a generated spec;
::                  point it into build\ so the project root
::                  keeps exactly one build file (this script)
::   excludes       env-pollution insurance only (Pillow probes
::                  Qt); no stdlib excludes - saving ~0.2 MB is
::                  not worth a lazy-import break. The macOS
::                  backend modules are never imported on Windows.
::   PIL_EXCLUDES   Pillow's image-file readers and writers and
::                  their native codecs (AVIF, WebP, colour
::                  management - several MB): the app draws its
::                  images and opens one file, app.ico, with the
::                  ICO reader (PNG and BMP frames), which stays.
::                  Pillow skips readers it cannot import.
::   hooks          PyInstaller's runtime hook for inspect imports
::                  inspect and zipfile at every launch (about
::                  1 MB that stays loaded) to fix up source-file
::                  lookups of frozen modules; the app never reads
::                  its sources. An empty hook of the same name in
::                  build\hooks takes its place.
::   Licenses.txt   every license the exe carries, from this
::                  build's Python; About > Legal notices shows it
::   _cffi_backend  PyNaCl's libsodium core imports it from C, where
::                  PyInstaller cannot see it, and the nacl hook does not
::                  add it: without it chat would be off
::   window modules are imported on first use (the tray loads none
::                  at startup); PyInstaller finds those imports, and
::                  step 9 checks that every one was packaged
:: ------------------------------------------------------------
echo.
echo [8/9] Building executable...

set "PIL_EXCLUDES="
for %%M in (AvifImagePlugin BlpImagePlugin BufrStubImagePlugin CurImagePlugin DcxImagePlugin DdsImagePlugin EpsImagePlugin FitsImagePlugin FliImagePlugin FpxImagePlugin FtexImagePlugin GbrImagePlugin GifImagePlugin GribStubImagePlugin Hdf5StubImagePlugin IcnsImagePlugin ImImagePlugin ImtImagePlugin IptcImagePlugin Jpeg2KImagePlugin JpegImagePlugin JpegPresets McIdasImagePlugin MicImagePlugin MpegImagePlugin MpoImagePlugin MspImagePlugin PalmImagePlugin PcdImagePlugin PcxImagePlugin PdfImagePlugin PdfParser PixarImagePlugin PpmImagePlugin PsdImagePlugin QoiImagePlugin SgiImagePlugin SpiderImagePlugin SunImagePlugin TgaImagePlugin TiffImagePlugin WebPImagePlugin WmfImagePlugin XVThumbImagePlugin XbmImagePlugin XpmImagePlugin ImageCms ImageDraw2 ImagePath ImageShow ImageWin features _avif _webp _imagingcms _imagingmath _imagingmorph) do set "PIL_EXCLUDES=!PIL_EXCLUDES! --exclude-module PIL.%%M"

if not exist build mkdir build
set "HOOKS=!CD!\build\hooks"
if not exist "!HOOKS!\rthooks" mkdir "!HOOKS!\rthooks"
> "!HOOKS!\rthooks.dat" echo {"inspect": ["pyi_rth_inspect.py"]}
> "!HOOKS!\rthooks\pyi_rth_inspect.py" echo # Generated by build_exe.bat: no inspect fix-up at startup.
!PY! collect_licenses.py "!CD!\build\Licenses.txt"
if errorlevel 1 (
    echo       ERROR: Could not collect the license texts.
    goto :DONE
)

!PY! -m PyInstaller --clean --noconfirm %MODE% --windowed --name WeekNumber ^
    !ICON_ARG! ^
    --add-data "!CD!\holiday_data.jsonl;." ^
    --add-data "!CD!\timezone_data.json;." ^
    --add-data "!CD!\tzd_data.json;." ^
    --add-data "!CD!\build\Licenses.txt;." ^
    --version-file "!VERFILE!" ^
    --optimize 2 --noupx --specpath build ^
    --additional-hooks-dir "!HOOKS!" ^
    --hidden-import _cffi_backend ^
    --exclude-module PIL.ImageQt ^
    --exclude-module PyQt5 --exclude-module PyQt6 ^
    --exclude-module PySide2 --exclude-module PySide6 ^
    --exclude-module IPython --exclude-module matplotlib ^
    --exclude-module numpy --exclude-module pandas ^
    --exclude-module weeknumber_menubar ^
    --exclude-module weeknumber_platform._macos ^
    --exclude-module weeknumber_platform._macos_tz ^
    !PIL_EXCLUDES! ^
    weeknumber.py

if errorlevel 1 (
    echo       ERROR: Build failed.
    goto :DONE
)

echo       OK

:: ------------------------------------------------------------
:: [9/9] Verify output
:: ------------------------------------------------------------
echo.
echo [9/9] Verifying output...

if not exist "%OUTEXE%" (
    echo       ERROR: %OUTEXE% not found.
    goto :DONE
)

set "ARCHIVE_LIST=!CD!\build\archive_contents.txt"
!PY! -m PyInstaller.utils.cliutils.archive_viewer -l -r "%OUTEXE%" > "!ARCHIVE_LIST!" 2>nul
if errorlevel 1 (
    echo       ERROR: Could not inspect the packaged application archive.
    goto :DONE
)
:: Chat's native parts (PyNaCl's libsodium core and cffi's backend) must
:: be packaged in every build, x64 and ARM64 alike.
!PY! -c "import nacl.bindings" >nul 2>nul
if errorlevel 1 (
    echo       ERROR: PyNaCl is missing; install requirements-release.lock.
    goto :DONE
)
for %%R in (weeknumber_calendar weeknumber_timezone_dialog weeknumber_holiday_dialog weeknumber_about weeknumber_license_dialog weeknumber_color_dialog weeknumber_tzd_lobby weeknumber_tzd_dialog weeknumber_tzd_chat weeknumber_tzd_sound weeknumber_platform._windows weeknumber_platform._wintray _sodium _cffi_backend PIL.IcoImagePlugin PIL.PngImagePlugin PIL.BmpImagePlugin _imagingft _imagingtk) do (
    findstr /i /c:"%%R" "!ARCHIVE_LIST!" >nul
    if errorlevel 1 (
        echo       ERROR: Required packaged module %%R is missing.
        goto :DONE
    )
)
if defined MODE (
    for %%R in (holiday_data.jsonl timezone_data.json tzd_data.json app.ico Licenses.txt) do (
        findstr /i /c:"%%R" "!ARCHIVE_LIST!" >nul
        if errorlevel 1 (
            echo       ERROR: Required packaged resource %%R is missing.
            goto :DONE
        )
    )
) else (
    for %%R in (holiday_data.jsonl timezone_data.json tzd_data.json app.ico Licenses.txt) do (
        if not exist "dist\WeekNumber\_internal\%%R" if not exist "dist\WeekNumber\%%R" (
            echo       ERROR: Required onedir resource %%R is missing.
            goto :DONE
        )
    )
)
echo       OK - required data, icon and dialog modules are packaged.

:: What the size is made of: the ten largest packaged items.
if defined MODE (
    echo       Largest packaged items [compressed KB]:
    !PY! -c "import ast, sys; rows = [ast.literal_eval('(' + l.strip() + ')') for l in open(sys.argv[1]) if l.lstrip()[:1].isdigit()]; rows = [r for r in rows if len(r) == 6]; [print('        %%6d  %%s' %% (r[1] // 1024, r[5])) for r in sorted(rows, key=lambda r: -r[1])[:10]]" "!ARCHIVE_LIST!"
)

for %%F in ("%OUTEXE%") do (
    echo       OK - %%~fF
    echo       Size: %%~zF bytes
)

set "BUILD_OK=1"

:: ------------------------------------------------------------
:DONE
echo.
echo ============================================================

if "%BUILD_OK%"=="1" (
    echo  SUCCESS: Build completed.
    echo  Output: %OUTEXE%   !ARCH!, Windows 10 1703+
    echo.
    echo  Recommendation:
    echo  - Code-sign the EXE for trust with SmartScreen
    echo  - Install under Program Files for proper ACL protection
) else (
    echo  FAILED: See errors above.
)

echo ============================================================
echo.
if "%NOPAUSE%"=="0" pause

if "%BUILD_OK%"=="1" (exit /b 0) else (exit /b 1)
