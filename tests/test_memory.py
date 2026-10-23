"""Windows' modules leave memory with the last window (ModuleSweeper), and the
release pass runs the sweep only when the app is idle. Tk-free."""

import gc
import logging
import sys
import textwrap
import weakref

import pytest

import weeknumber_core as core


@pytest.fixture
def scratch_modules(tmp_path, monkeypatch):
    """A directory of throwaway modules on sys.path; removes what it loaded."""
    monkeypatch.syspath_prepend(str(tmp_path))
    before = set(sys.modules)

    def write(name: str, source: str) -> None:
        path = tmp_path.joinpath(*name.split("."))
        if "." in name:
            path.parent.mkdir(parents=True, exist_ok=True)
            init = path.parent / "__init__.py"
            if not init.exists():
                init.write_text("", encoding="utf-8")
        path.with_suffix(".py").write_text(textwrap.dedent(source), encoding="utf-8")

    yield write
    for name in set(sys.modules) - before:
        sys.modules.pop(name, None)


def test_sweep_unloads_what_windows_imported_and_nothing_else(scratch_modules):
    scratch_modules("wn_probe_window", "VALUE = 1\n")
    sweeper = core.ModuleSweeper()
    sweeper.mark()
    import wn_probe_window  # noqa: F401  (a window's module, after startup)
    assert "wn_probe_window" in sweeper.candidates()
    assert sweeper.sweep() == ["wn_probe_window"]
    assert "wn_probe_window" not in sys.modules
    assert "weeknumber_core" in sys.modules  # the baseline stays
    import wn_probe_window as again  # and it imports again cleanly
    assert again.VALUE == 1


def test_unload_hook_runs_before_the_module_goes(scratch_modules):
    scratch_modules("wn_probe_hook", """
        CALLS = []
        def __weeknumber_unload__():
            CALLS.append("unloaded")
    """)
    sweeper = core.ModuleSweeper()
    sweeper.mark()
    import wn_probe_hook
    calls = wn_probe_hook.CALLS
    sweeper.sweep()
    assert calls == ["unloaded"]


def test_kept_names_and_native_packages_stay(scratch_modules, monkeypatch):
    sweeper = core.ModuleSweeper()
    sweeper.mark()
    scratch_modules("wn_probe_pkg.part", "X = 1\n")
    import wn_probe_pkg.part  # noqa: F401  (imported for its side effect)
    # A package with a native part is pinned whole (here: a stand-in whose
    # loader is not a source loader, as for an extension module).

    class NativeLoader:
        pass

    native = type(sys)("wn_probe_pkg._native")
    native.__spec__ = type("Spec", (), {"loader": NativeLoader()})()
    monkeypatch.setitem(sys.modules, "wn_probe_pkg._native", native)
    assert not any(name.startswith("wn_probe_pkg") for name in sweeper.candidates())
    assert core.ModuleSweeper._kept_prefix("tkinter.ttk")
    assert core.ModuleSweeper._kept_prefix("PIL.ImageTk")
    assert core.ModuleSweeper._kept_prefix("pyimod02_importers")
    assert not core.ModuleSweeper._kept_prefix("weeknumber_calendar")


def test_annotations_do_not_keep_an_unloaded_module_alive(scratch_modules):
    # typing caches Optional[Session] & co.; without clearing them every
    # reload kept another copy of the module's namespace.
    scratch_modules("wn_probe_typed", """
        from typing import Optional, overload
        class Session:
            pass
        def connect(session: Optional[Session] = None) -> Optional[Session]:
            return session
        @overload
        def pick(value: int) -> int: ...
        @overload
        def pick(value: str) -> str: ...
        def pick(value):
            return value
    """)
    sweeper = core.ModuleSweeper()
    sweeper.mark()
    import wn_probe_typed
    namespace = weakref.ref(wn_probe_typed.Session)
    del wn_probe_typed
    sweeper.sweep()
    gc.collect()
    assert namespace() is None


def test_a_library_null_handler_does_not_pile_up(scratch_modules):
    scratch_modules("wn_probe_logs", """
        import logging
        logging.getLogger("wn_probe_logs").addHandler(logging.NullHandler())
    """)
    sweeper = core.ModuleSweeper()
    logger = logging.getLogger("wn_probe_logs")
    for _ in range(3):
        sweeper.mark()
        sweeper._baseline.discard("wn_probe_logs")
        import wn_probe_logs  # noqa: F401  (imported for its side effect)
        sweeper.sweep()
    assert logger.handlers == []


class _FakeRoot:
    def __init__(self):
        self.timers = {}

    def nametowidget(self, name):
        return self

    def after(self, ms, func):
        timer = f"after#{len(self.timers) + 1}"
        self.timers[timer] = func
        return timer

    def after_cancel(self, timer):
        del self.timers[timer]


@pytest.mark.parametrize("idle", [True, False])
def test_release_pass_sweeps_and_trims_only_when_idle(monkeypatch, idle):
    root = _FakeRoot()
    calls = []
    monkeypatch.setattr(core, "_release_after", None)
    monkeypatch.setattr(core, "_idle_hooks", [])
    monkeypatch.setattr(core.platform_, "trim_memory", lambda: calls.append("heap"))
    monkeypatch.setattr(core.platform_, "trim_working_set", lambda: calls.append("working set"))
    core.set_idle_probe(lambda: idle)
    core.add_idle_hook(lambda: calls.append("sweep"))
    try:
        core.release_memory_soon(root)
        (release,) = root.timers.values()
        release()
    finally:
        core.set_idle_probe(None)
    assert calls == (["sweep", "heap", "working set"] if idle else ["heap"])


def test_a_failing_idle_probe_counts_as_busy(monkeypatch):
    def broken():
        raise RuntimeError("boom")
    core.set_idle_probe(broken)
    try:
        assert core._is_idle() is False
    finally:
        core.set_idle_probe(None)
