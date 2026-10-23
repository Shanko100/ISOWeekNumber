"""WeekNumber launcher and application lifecycle."""

import atexit
import time

# weeknumber_core first: on Windows its platform layer restricts the DLL
# search path before tkinter loads Tcl/Tk.
import weeknumber_core as core
import tkinter as tk
from weeknumber_core import (
    APP_NAME,
    bundled_file,
    APP_VERSION,
    ICO_WINDOW_ICONS,
    QuitSignal,
    SettingsStore,
    SingleInstance,
    UiDispatcher,
    enable_dpi_awareness,
    holiday_countries,
    log,
    network,
    prepare_app,
    refresh_startup_path,
    release_memory_soon,
    set_app_identity,
    setup_logging,
    set_startup,
    show_error,
    system_dpi,
)
from weeknumber_tray import TrayIcon


class Application:
    REFRESH_MS = 60_000
    #: Eviction handover budget -- how long a launch waits for the incumbent
    #: to drop the mutex before giving up.
    HANDOVER_TIMEOUT_S = 5.0
    HANDOVER_POLL_S = 0.1

    def __init__(self) -> None:
        self.single_instance = SingleInstance()
        self.quit_signal = QuitSignal()
        self.settings = SettingsStore()
        # Only the index is read here; the calendar loads the years it shows.
        # The saved time zone is checked when a window first needs the zone
        # table (weeknumber_timezone.use_zones), not at startup.
        self.settings.validate_holiday_countries(
            {code for code, _name in holiday_countries()}
        )
        if not self.settings.startup_initialized:
            # A genuine first run starts with autostart off. A settings file
            # that exists but can't be read must not wipe the user's choice
            # on every launch, so only a missing file triggers the reset.
            if not self.settings.file_existed:
                set_startup(False)
            self.settings.mark_startup_initialized()
        self.tray = TrayIcon(self)
        self.root: tk.Tk | None = None
        self.dispatcher: UiDispatcher | None = None
        self.running = True

    def _apply_default_icon(self) -> None:
        """Register app.ico as the default icon for every Tk window, so the
        time-zone dialog shows the app's own icon in its caption and on the
        taskbar rather than Tk's feather."""
        if not ICO_WINDOW_ICONS:
            return
        icon = bundled_file("app.ico")
        if icon is None or self.root is None:
            return
        try:
            self.root.iconbitmap(default=str(icon))
        except tk.TclError:
            log.warning("Could not apply the default window icon")

    def run_on_ui(self, func) -> None:
        """Marshal a callable onto the Tk main thread from any thread; every
        Tk object is then only ever touched on the main thread (see
        UiDispatcher for why this is a queue rather than a foreign `after`)."""
        if self.dispatcher is not None:
            self.dispatcher.post(func)

    def request_shutdown(self) -> None:
        self.run_on_ui(self._shutdown)

    def _shutdown(self) -> None:
        if not self.running:
            return
        self.running = False
        self.tray.stop()
        if self.root is not None:
            self.root.quit()

    def _periodic_refresh(self) -> None:
        # Minute tick: recomputes the week number and repaints the tray icon
        # only when its inputs changed. Reschedules itself.
        if not self.running or self.root is None:
            return
        try:
            self.tray.week.refresh()
            self.tray.refresh()
        except Exception:
            log.exception("Periodic refresh failed")
        self.root.after(self.REFRESH_MS, self._periodic_refresh)

    def _claim_singleton(self) -> bool:
        """Take the single-instance mutex, evicting any incumbent first.

        Launching the app -- from VS Code or the .exe -- replaces the running
        copy: signal it to quit, then poll until it releases the mutex and we
        can take it. No external process kill: the incumbent tears its own
        tray/Tk down cleanly, which also avoids the leftover ghost tray icon a
        forced TerminateProcess would leave behind."""
        if self.single_instance.acquire():
            return True
        log.info("Existing instance detected; requesting handover")
        self.quit_signal.signal()
        deadline = time.monotonic() + self.HANDOVER_TIMEOUT_S
        while time.monotonic() < deadline:
            time.sleep(self.HANDOVER_POLL_S)
            if self.single_instance.acquire():
                log.info("Handover complete; this instance is now primary")
                return True
        show_error(f"{APP_NAME} is already running and did not respond in time. "
                   f"Quit it from Task Manager or Activity Monitor, then relaunch.")
        return False

    def run(self) -> int:
        if not self._claim_singleton():
            return 0
        atexit.register(self.single_instance.release)
        atexit.register(self.quit_signal.release)

        refresh_startup_path()

        self.root = tk.Tk()
        self.root.withdraw()
        prepare_app(self.root)
        self.dispatcher = UiDispatcher(self.root)
        self.dispatcher.start()
        self._apply_default_icon()
        self.root.protocol("WM_DELETE_WINDOW", lambda: None)
        # Incumbent side of the handover. Started once root exists so the
        # wake callback can marshal a shutdown onto the Tk thread.
        self.quit_signal.host(self.request_shutdown)

        self.tray.run()
        self.root.after(self.REFRESH_MS, self._periodic_refresh)
        # Everything loaded so far is what the idle tray needs; modules the
        # windows bring in later are unloaded again once they all close.
        self.tray.start_memory_management()
        release_memory_soon(self.root)  # what starting up left behind
        log.info("%s %s started (tray icon %dpx, system DPI %d)",
                 APP_NAME, APP_VERSION, self.tray.icon_size, system_dpi())
        try:
            self.root.mainloop()
        finally:
            self.tray.stop()  # idempotent; covers abnormal mainloop exits
            try:
                self.root.destroy()
            except tk.TclError:
                pass
            self.quit_signal.release()
            self.single_instance.release()
        return 0



def main() -> int:
    # First: from here on no Internet connection is possible unless the
    # player allows one for Timezone Dominator (see weeknumber_core.NetworkGate).
    network.install()
    setup_logging()
    enable_dpi_awareness()  # before Tk and before any DPI query
    set_app_identity()      # before the first window: the taskbar groups by it
    try:
        return Application().run()
    except Exception:
        log.exception("Fatal error")
        show_error(f"{APP_NAME} encountered an unexpected error and needs to close. "
                   f"Details were written to the log file.")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
