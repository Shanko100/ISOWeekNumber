"""Operating-system layer for WeekNumber.

Everything that differs between Windows and macOS lives behind this package:
DPI and monitor geometry, window chrome, the single-instance guard and its
eviction signal, run-at-startup registration, the settings location and the
fonts the tray icon is drawn with. The rest of the app imports these names
from ``weeknumber_core``, which re-exports them, so no UI module needs to know
which backend is active.

Import this before anything that loads a native extension: on Windows the
backend restricts the DLL search path to System32 as its very first action.
"""

import sys

IS_WINDOWS = sys.platform == "win32"
IS_MACOS = sys.platform == "darwin"

if IS_WINDOWS:
    from weeknumber_platform._windows import *  # noqa: F401,F403
    from weeknumber_platform._windows import __all__ as _backend_names
elif IS_MACOS:
    from weeknumber_platform._macos import *  # noqa: F401,F403
    from weeknumber_platform._macos import __all__ as _backend_names
else:
    print("WeekNumber runs on Windows and macOS only.")
    sys.exit(1)

__all__ = ["IS_WINDOWS", "IS_MACOS", *_backend_names]
