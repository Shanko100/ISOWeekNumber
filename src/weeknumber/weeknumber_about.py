"""About window for WeekNumber."""

import tkinter as tk
import webbrowser

from PIL import Image, ImageOps, ImageTk

from weeknumber_core import (
    APP_NAME,
    APP_URL,
    APP_VERSION,
    RECT,
    SettingsStore,
    bring_app_to_front,
    bundled_file,
    log,
)
from weeknumber_dialog import TitledDialog
from weeknumber_license_dialog import licenses_available, show_licenses
from weeknumber_widgets import PillButton


class AboutPopup(TitledDialog):
    """Fixed About dialog: a real titled window, not a flyout.

    It stays open until closed -- close button, Alt+F4, or Esc; there is no
    focus-loss dismissal. Centred in the work area of the monitor the cursor
    is on, with the client area sized in physical pixels from that monitor's
    effective DPI, so the dialog has the same physical size on every display.
    Single instance: reopening focuses the open window."""

    #: Base client-area metrics in px @ 96 DPI; every value below is scaled
    #: by the target monitor's effective DPI at build time.
    WIDTH_PX = 380
    HEIGHT_PX = 180
    PAD_PX = 28                    # left/right content padding
    ICON_PX = 80                   # app-icon square
    ICON_TEXT_GAP_PX = 20
    NAME_VERSION_GAP_PX = 4
    BUTTON_H_PX = 36
    BUTTON_MARGIN_BOTTOM_PX = 16

    def __init__(self, master: tk.Misc, settings: "SettingsStore") -> None:
        work = self._init_window(master, settings)
        self.top.title(f"About {APP_NAME}")
        # Fully transparent until final placement: the decorated frame can
        # only be measured after the first map (see _center_outer); alpha 0
        # keeps that invisible instead of flashing a jumping window.
        self.top.attributes("-alpha", 0.0)
        self.card: "tk.Frame | None" = None
        self._icon_img: "ImageTk.PhotoImage | None" = None
        self._build_card()

        # Provisional centre using the client size; corrected for the real
        # frame after the first map.
        client_w, client_h = self._px(self.WIDTH_PX), self._px(self.HEIGHT_PX)
        x = work.left + max(0, (work.right - work.left - client_w) // 2)
        y = work.top + max(0, (work.bottom - work.top - client_h) // 2)
        self.top.geometry(f"{client_w}x{client_h}+{x}+{y}")
        self.top.deiconify()
        self._sync_titlebar()
        self._center_outer(work)
        self.top.attributes("-alpha", 1.0)
        bring_app_to_front()
        self.top.lift()
        self.top.focus_force()

    # -- construction ---------------------------------------------------------
    def _build_card(self) -> None:
        """(Re)create the client-area content for the active theme. Shared by
        first construction and dark-mode refreshes: the icon and the button
        pill are palette-baked images, so a theme change must rebuild, not
        repaint (same contract as CalendarPopup._build_card)."""
        self.top.configure(bg=self._color("CARD_BG"))
        if self.card is not None:
            self.card.destroy()
        self._icon_img = None

        # Fixed dialog: the client size is pinned by wm geometry plus
        # resizable(False, False), and every child is place()-managed, so
        # content can never resize the window.
        client_w, client_h = self._px(self.WIDTH_PX), self._px(self.HEIGHT_PX)
        self.card = tk.Frame(self.top, bg=self._color("CARD_BG"),
                             width=client_w, height=client_h)
        self.card.pack(fill="both", expand=True)

        pad = self._px(self.PAD_PX)
        btn_h = self._px(self.BUTTON_H_PX)
        btn_y = client_h - btn_h - self._px(self.BUTTON_MARGIN_BOTTOM_PX)

        # Icon + name/version block, vertically centred in the area above the
        # button: anchor "w" centres the row's own height on btn_y / 2, which
        # is exactly (btn_y - block_height) / 2 for the block's top edge.
        row = tk.Frame(self.card, bg=self._color("CARD_BG"))
        row.place(x=pad, y=btn_y // 2, anchor="w")

        self._icon_img = self._load_icon_image(self._px(self.ICON_PX))
        if self._icon_img is not None:
            tk.Label(row, image=self._icon_img, bg=self._color("CARD_BG"),
                     bd=0, highlightthickness=0).pack(side="left")

        text = tk.Frame(row, bg=self._color("CARD_BG"))
        text.pack(side="left", padx=(
            self._px(self.ICON_TEXT_GAP_PX) if self._icon_img is not None else 0, 0))
        tk.Label(text, text=APP_NAME, font=self._font(20, bold=True),
                 bg=self._color("CARD_BG"), fg=self._color("TITLE_FG"),
                 bd=0, highlightthickness=0).pack(anchor="w")
        version = tk.Frame(text, bg=self._color("CARD_BG"))
        version.pack(anchor="w", padx=(self._px(2), 0),  # optical left-align with name
                     pady=(self._px(self.NAME_VERSION_GAP_PX), 0))
        tk.Label(version, text=f"Version {APP_VERSION}", font=self._font(12),
                 bg=self._color("CARD_BG"), fg=self._color("YEAR_FG"),
                 bd=0, highlightthickness=0).pack(side="left")
        # Builds carry Licenses.txt (collect_licenses.py); a source run shows
        # the repository's texts instead (see weeknumber_license_dialog).
        if licenses_available():
            tk.Label(version, text="  ·  ", font=self._font(12),
                     bg=self._color("CARD_BG"), fg=self._color("YEAR_FG"),
                     bd=0, highlightthickness=0).pack(side="left")
            link = tk.Label(version, text="Legal notices", font=self._font(12),
                            bg=self._color("CARD_BG"), fg=self._color("ACCENT"),
                            bd=0, highlightthickness=0, cursor="hand2")
            link.bind("<Button-1>", lambda _e: self._open_licenses())
            link.pack(side="left")

        # 'View on GitHub': the app's standard secondary pill, full width,
        # centred in its slot above the bottom margin.
        button = PillButton(self.card, "View on GitHub  ↗", self._font(12),
                            self.scale, self._open_github, primary=False,
                            min_width_px=self.WIDTH_PX - 2 * self.PAD_PX)
        button.apply_theme(self.theme)
        button.place(x=pad, y=btn_y + btn_h // 2, anchor="w")

    def _load_icon_image(self, size: int) -> "ImageTk.PhotoImage | None":
        """app.ico scaled to `size` px: largest embedded frame, LANCZOS,
        letterboxed on transparency if non-square. Returns None -- and the
        header lays out text-only -- when the file is unavailable."""
        path = bundled_file("app.ico")
        if path is None:
            log.info("app.ico not found; About window renders without an icon")
            return None
        try:
            # Only the ICO reader (with the PNG and BMP frames inside it):
            # the build ships no other image parsers, and none is needed.
            with Image.open(path, formats=["ICO"]) as ico:
                src = ico.convert("RGBA")  # forces load of the largest frame
            src = ImageOps.contain(src, (size, size), Image.LANCZOS)
            out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
            out.paste(src, ((size - src.width) // 2, (size - src.height) // 2), src)
            return ImageTk.PhotoImage(out, master=self.top)
        except Exception:
            log.exception("Could not load app.ico for the About window")
            return None

    # -- interaction & lifetime -------------------------------------------------
    def _open_github(self) -> None:
        # Same contract as elsewhere: webbrowser swallows the OS error and
        # returns False, hence the explicit check.
        if not webbrowser.open(APP_URL):
            log.warning("Could not open %s in a browser", APP_URL)

    def _open_licenses(self) -> None:
        """The Legal notices window, in the app's own style, opened beside
        this one; About stays open behind it."""
        if not show_licenses(self.top.nametowidget("."), self.settings, near=self.top):
            log.warning("No license texts to show")

    def _apply_theme(self) -> None:
        old = self.theme
        super()._apply_theme()
        if self.theme != old:
            self._build_card()  # geometry untouched: same monitor, same scale
        self.top.lift()

    def _center_outer(self, work: RECT) -> None:
        """Centre the OUTER frame in the work area. wm geometry positions the
        outer frame but sizes the client area, so the provisional centre in
        __init__ ignores the title bar and borders; once mapped, the real
        decoration offsets are measurable (winfo_rootx/rooty = client origin,
        winfo_x/y = frame origin) and the position is corrected. The window
        is still alpha-0 here, so the shift is never visible."""
        self.top.update_idletasks()
        border = self.top.winfo_rootx() - self.top.winfo_x()
        caption = self.top.winfo_rooty() - self.top.winfo_y()
        outer_w = self.top.winfo_width() + 2 * border
        outer_h = self.top.winfo_height() + caption + border  # bottom == side
        x = work.left + max(0, (work.right - work.left - outer_w) // 2)
        y = work.top + max(0, (work.bottom - work.top - outer_h) // 2)
        self.top.geometry(f"+{x}+{y}")
