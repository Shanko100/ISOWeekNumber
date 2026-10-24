"""Calendar popup UI for WeekNumber."""

import datetime as dt
import time
import tkinter as tk
import tkinter.font as tkfont
from typing import TYPE_CHECKING

try:
    from PIL import Image, ImageChops, ImageDraw, ImageOps, ImageTk
except ImportError:
    import sys
    print("Missing dependency: Pillow\nInstall with: pip install pillow")
    sys.exit(1)

from weeknumber_core import (
    CLOCK_TICK_MS,
    CONTEXT_MENU_EVENTS,
    GLASS_BACKDROP,
    POINT,
    RECT,
    UI_CACHE,
    UI_FONT_FAMILIES,
    SettingsStore,
    attach_glass_backdrop,
    bring_app_to_front,
    cursor_monitor_geometry,
    hold_holidays,
    holiday_country_name,
    holiday_names,
    log,
    owns_foreground,
    release_holidays,
    release_memory_soon,
    set_rounded_corners,
    theme_colors,
    toplevel_hwnd,
    tray_edge,
)
from weeknumber_clipboard import format_calendar_week, format_gui_date, format_iso_week
from weeknumber_widgets import PillButton
from weeknumber_timezone import (
    drop_zones,
    format_offset_short,
    resolve_zone_key,
    use_clock,
    zone_now_and_offset,
)

if TYPE_CHECKING:
    from collections.abc import Callable
    from PIL.ImageTk import PhotoImage as TkPhotoImage


def ui_font_family(widget: tk.Misc) -> str:
    """Resolved UI font family for any WeekNumber window.

    Cached process-wide in weeknumber_core.UI_CACHE (the installed font set
    does not change at runtime, and this module is unloaded while no window
    is open). The candidates are per platform (UI_FONT_FAMILIES); each is
    probed because Tk silently substitutes an arbitrary default for a missing
    family, which looks worse than the toolkit's own default face."""
    family = UI_CACHE.get("font_family")
    if family is None:
        try:
            families = set(tkfont.families(widget))
        except tk.TclError:
            families = set()
        family = next((f for f in UI_FONT_FAMILIES if f in families), None)
        if family is None:
            try:
                family = tkfont.nametofont("TkDefaultFont", widget).actual("family")
            except tk.TclError:
                family = UI_FONT_FAMILIES[-1]
        UI_CACHE["font_family"] = family
    return family


class CalendarPopup:
    """Borderless two-month calendar, anchored to the taskbar edge of the
    monitor the cursor is on and scaled to that monitor's effective DPI.

    Built fully while withdrawn (no first-paint flash); the label grid is
    created once and month navigation only reconfigures text/colours."""

    MONTHS_SHOWN = 2
    MIN_MONTH = dt.date(2000, 1, 1)
    # The first pane stops at November so the second pane is December 2050.
    MAX_MONTH = dt.date(2050, 11, 1)
    ROWS = 6
    #: Day-cell box size in px @ 96 DPI. Floors the today circle's diameter.
    #: 25 gives the day numbers a little more breathing room without turning
    #: the two-month flyout into a large wall calendar.
    CELL_PX = 25
    #: Vertical gap between the month/year title bar and weekday headers, in px
    #: @ 96 DPI. 6px follows the Fluent 8px-ish rhythm of the rest of the
    #: card; the previous 4px read as crowded next to the larger card padding.
    TITLE_WEEKDAY_GAP_PX = 6
    #: Hover dwell before a holiday-name tooltip appears, in milliseconds.
    TOOLTIP_DELAY_MS = 450
    #: Card padding in px @ 96 DPI; also the chips' inset for concentricity.
    CARD_PAD_PX = 10
    #: A click on the Windows tray icon takes the focus first, which closes
    #: the open flyout; a click this soon after such a close is that click.
    TOGGLE_GRACE_S = 0.4
    _instance: "CalendarPopup | None" = None
    #: When the flyout last closed because it lost the focus (monotonic).
    _dismissed_at = float("-inf")
    #: The root the grid's class bindings live on (see _bind_grid_classes).
    _bound_root: "tk.Misc | None" = None

    @classmethod
    def _live(cls) -> "CalendarPopup | None":
        inst = cls._instance
        try:
            if inst is not None and inst.top.winfo_exists():
                return inst
        except tk.TclError:
            pass
        cls._instance = None
        return None

    @classmethod
    def show(cls, master: tk.Misc, settings: "SettingsStore",
             on_timezone: "Callable[[], None] | None" = None) -> None:
        """Open a fresh flyout, replacing an open one."""
        old = cls._live()
        if old is not None:
            old.close()  # also closes its glass panel, which destroy() would orphan
        cls._instance = cls(master, settings, on_timezone)

    @classmethod
    def toggle(cls, master: tk.Misc, settings: "SettingsStore",
               on_timezone: "Callable[[], None] | None" = None) -> None:
        """The tray icon and menu-bar number are a switch: a click opens the
        flyout, the next one closes it."""
        open_now = cls._live()
        if open_now is not None:
            open_now.close()
        elif time.monotonic() - cls._dismissed_at >= cls.TOGGLE_GRACE_S:
            cls.show(master, settings, on_timezone)

    @classmethod
    def refresh_open(cls) -> None:
        """Re-render the open popup in place after settings-backed display data
        changes. Usually a no-op: opening the tray menu drops the popup's focus,
        which closes it first. Still keep the live path correct because tray
        menu focus behaviour varies across Windows versions."""
        inst = cls._instance
        if inst is None:
            return
        try:
            if inst.top.winfo_exists():
                inst._refresh_from_settings()
        except tk.TclError:
            pass

    def __init__(self, master: tk.Misc, settings: "SettingsStore",
                 on_timezone: "Callable[[], None] | None" = None) -> None:
        cursor, monitor, work, scale = cursor_monitor_geometry()
        self.scale = scale
        self.settings = settings
        # Supplied by the tray, which owns both windows. Keeping the dialog out
        # of this module's imports avoids a cycle: the dialog imports from here.
        self._on_timezone = on_timezone
        self.theme = theme_colors(settings)
        self._work = work
        today_month = dt.date.today().replace(day=1)
        self.display_month = min(self.MAX_MONTH, max(self.MIN_MONTH, today_month))
        self.selected_date: dt.date | None = None
        self._had_focus = False
        self._tip: "tk.Toplevel | None" = None
        self._tip_after = None
        self._tip_visible = False
        self._context_menu_visible = False
        self._clock_after = None
        self._clock_text = None
        self._zones_held = False
        # macOS: the flyout floats on system glass (Liquid Glass where
        # available), as Apple recommends for panels over other content.
        self._glass = GLASS_BACKDROP
        self._backdrop = None
        self._tip_backdrop = None
        self._overlay_pending = False
        self._decor_widgets: list[tk.Widget] = []

        self.top = tk.Toplevel(master)
        self.top.withdraw()
        self.top.overrideredirect(True)
        self.top.attributes("-topmost", True)
        if self._glass:
            self.top.wm_attributes("-transparent", True)
        self._family = ui_font_family(self.top)

        self.card: "tk.Frame | None" = None
        self._build_card()

        bindings = (
            ("<Escape>", lambda e: self.close()),
            ("<FocusOut>", lambda e: None if (self._tip_visible or self._context_menu_visible) else self._dismiss()),
            ("<MouseWheel>", self._on_mousewheel),
            ("<Left>", lambda e: self._shift_month(-1)),
            ("<Right>", lambda e: self._shift_month(1)),
            ("<Home>", lambda e: self._go_today()),
        )
        for sequence, handler in bindings:
            self.top.bind(sequence, handler)

        self._place(cursor, monitor, work)
        self.top.deiconify()
        bring_app_to_front()
        self.top.lift()
        self.top.focus_force()

        self.top.update_idletasks()
        self._hwnd = toplevel_hwnd(self.top)
        set_rounded_corners(self._hwnd)
        if self._glass:
            self._attach_glass(cursor, monitor, work)
        self.top.after(200, self._watch_foreground)
        self._tick_clock()

    # -- glass (macOS) ---------------------------------------------------------
    def _corner_radius(self) -> float:
        """The panel radius that makes the footer chips concentric with the
        panel's corners: chip radius plus the chip's inset from the edge (the
        1px stroke slot and the card padding)."""
        inset = 1 + self._px(self.CARD_PAD_PX)
        return self._footer.winfo_reqheight() / 2.0 + inset

    def _attach_glass(self, cursor, monitor, work) -> None:
        self._backdrop = attach_glass_backdrop(
            self.top, self._corner_radius(), self.settings.is_dark_mode)
        if self._backdrop is None:
            # No glass after all: rebuild on the opaque card instead.
            log.info("Glass backdrop unavailable; using the opaque flyout")
            self._glass = False
            self.top.wm_attributes("-transparent", False)
            self._build_card()
            self._place(cursor, monitor, work)
            return
        self.top.bind("<Configure>", self._on_configure, add="+")
        self._render_overlay()

    def _on_configure(self, event) -> None:
        if event.widget is self.top and self._backdrop is not None:
            self._backdrop.sync()

    def _color(self, name: str) -> str:
        return self.theme[name]

    def _build_card(self) -> None:
        """(Re)create the surface and everything on it for the active theme.

        Shared by first construction and by theme switches: the rounded
        PhotoImages are pre-rendered with palette colours, so a theme change
        must rebuild the widget tree, not just repaint it. The toplevel keeps
        the border colour and the card sits 1px inside it -- the Fluent
        hairline surface stroke; DWM rounds the stroke with the window."""
        # Opaque: the toplevel shows as the 1px surface stroke around the card.
        # Glass: no stroke; the glass draws its own edge.
        self.top.configure(bg=self._surface() if self._glass else self._color("CARD_BORDER"))
        if self.card is not None:
            self.card.destroy()
        pad = self._px(self.CARD_PAD_PX)
        self.card = tk.Frame(self.top, bg=self._surface(), padx=pad, pady=pad)
        self.card.pack(fill="both", expand=True, padx=1, pady=1)

        self._build_cell_images()
        self._build_skeleton()
        self._refresh()

    def _refresh_from_settings(self) -> None:
        """Refresh settings-driven UI state without losing the selected date.

        Holiday changes only need a data repaint. Theme changes are different:
        see _build_card.
        """
        new_theme = theme_colors(self.settings)
        if new_theme == self.theme:
            self._refresh()
            return

        cursor, monitor, work, scale = cursor_monitor_geometry()
        self.scale = scale
        self._work = work
        self.theme = new_theme
        self._hide_tip()

        self._build_card()
        self._place(cursor, monitor, work)
        if self._backdrop is not None:
            self._backdrop.set_dark(self.settings.is_dark_mode)
            self._backdrop.sync()
        self.top.lift()

    # -- metrics ------------------------------------------------------------
    def _px(self, value: float) -> int:
        return max(1, round(value * self.scale))

    def _font(self, px: int, bold: bool = False):
        # Negative Tk font size == physical pixels: per-monitor exact sizing,
        # independent of Tk's global (primary-monitor) point scaling.
        font = (self._family, -self._px(px))
        return font + ("bold",) if bold else font

    # -- construction -------------------------------------------------------
    #: Pixel ratio of the glass overlay's artwork: drawn natively by AppKit,
    #: so it can be Retina-sharp, unlike Tk's one-pixel-per-point images.
    GLASS_RATIO = 2
    #: Bind tags shared by the grid's week labels and day cells.
    WEEK_TAG = "WeekNumberWeek"
    DAY_TAG = "WeekNumberDay"
    #: The artwork for the current palette and scale, one entry per ratio,
    #: shared by a theme switch's rebuild; cleared when the popup closes.
    _art_cache: "dict[tuple, dict]" = {}

    def _cell_art(self, ratio: int = 1) -> dict:
        """The calendar's marker artwork as PIL images at `ratio` pixels per
        Tk pixel: masks, state layers, band notches, chevrons. Ratio 1 feeds
        the Tk images; the glass overlay draws a ratio-2 copy natively. The
        images are shared between opens, so callers must not draw on them."""
        key = (ratio, self.scale, tuple(sorted(self.theme.items())))
        cache = CalendarPopup._art_cache
        art = cache.get(key)
        if art is None:
            if any(k[1:] != key[1:] for k in cache):
                cache.clear()
            art = cache[key] = self._draw_cell_art(ratio)
        return art

    def _draw_cell_art(self, ratio: int) -> dict:
        s = self.scale * ratio

        def px(value: float) -> int:
            return max(1, round(value * s))

        d = px(self.CELL_PX)
        r = px(5)               # corner radius @ this DPI
        ss = 4                        # supersample, LANCZOS down for clean edges

        def mask(side: str) -> "Image.Image":
            # Full rounded mask, then square off the corners we do NOT want
            # by painting that edge strip solid. This avoids
            # rounded_rectangle's `corners=` argument, which only exists in
            # Pillow >= 9.4; this path works back to 8.2, when
            # rounded_rectangle itself was added. Output is identical.
            n, rad = d * ss, r * ss
            big = Image.new("L", (n, n), 0)
            dr = ImageDraw.Draw(big)
            dr.rounded_rectangle((0, 0, n - 1, n - 1), radius=rad, fill=255)
            if side == "left":        # keep left corners; square the right
                dr.rectangle((n - rad - 1, 0, n - 1, n - 1), fill=255)
            elif side == "right":     # keep right corners; square the left
                dr.rectangle((0, 0, rad, n - 1), fill=255)
            return big.resize((d, d), Image.LANCZOS)

        def ring_mask(width_px: float = 1.25) -> "Image.Image":
            # Outline-only rounded square, hugging the same silhouette as
            # the full pill (PIL strokes grow inward from the boundary).
            # Selection uses a quiet ~1.25px ring; holiday rings are 2px
            # so they remain legible around the solid Today marker.
            n, rad = d * ss, r * ss
            width = max(1, round(width_px * s)) * ss
            big = Image.new("L", (n, n), 0)
            ImageDraw.Draw(big).rounded_rectangle(
                (0, 0, n - 1, n - 1), radius=rad, outline=255, width=width)
            return big.resize((d, d), Image.LANCZOS)

        full = mask("full")
        left = mask("left")    # left pair only
        right = mask("right")  # right pair only
        ring = ring_mask()
        holiday_ring = ring_mask(2.0)

        def layer(m: "Image.Image", color: str) -> "Image.Image":
            # Solid colour behind an alpha mask; a reusable compose input.
            out = Image.new("RGBA", (d, d), color)
            out.putalpha(m)
            return out

        card = self._color("CARD_BG")
        # Inverse-alpha band caps: card-coloured corner cut-outs. Laid over
        # a bg=band cell they paint card into the band's square outer
        # corners, so the row stays one continuous band fill while its end
        # reads as rounded. Band colour is the cell bg, so a single card
        # notch serves both highlight colours.
        notch_left = layer(left.point(lambda px: 255 - px), card)
        notch_right = layer(right.point(lambda px: 255 - px), card)

        today = layer(full, self._color("ACCENT"))
        selected = layer(full, self._color("SELECTED_DAY_FILL"))
        sel_ring = layer(ring, self._color("ACCENT"))
        holiday = layer(full, self._color("HOLIDAY_BG"))
        holiday_secondary = layer(full, self._color("HOLIDAY_SECONDARY_BG"))

        def striped_holiday() -> "Image.Image":
            # Render at the same supersampled resolution as the rounded
            # mask, then clip once. This avoids seams at stripe edges and
            # keeps the outer silhouette identical to solid markers.
            n, rad = d * ss, r * ss
            big = Image.new("RGBA", (n, n), self._color("HOLIDAY_BG"))
            draw = ImageDraw.Draw(big)
            stripe = max(ss, round(4 * s * ss))
            for x in range(-n, 2 * n, stripe * 2):
                draw.line((x, 0, x - n, n),
                          fill=self._color("HOLIDAY_SECONDARY_BG"),
                          width=stripe)
            clip = Image.new("L", (n, n), 0)
            ImageDraw.Draw(clip).rounded_rectangle(
                (0, 0, n - 1, n - 1), radius=rad, fill=255)
            big.putalpha(clip)
            return big.resize((d, d), Image.LANCZOS)

        holiday_dual = striped_holiday()

        holiday_primary_ring = layer(
            holiday_ring, self._color("HOLIDAY_BORDER"))
        holiday_secondary_ring = layer(
            holiday_ring, self._color("HOLIDAY_SECONDARY_BORDER"))

        def striped_holiday_ring() -> "Image.Image":
            # The holiday state must survive Today/selection without
            # replacing those primary states. A saturated outer ring does
            # that cleanly; when both calendars contain a holiday, clip
            # diagonal purple/teal stripes to that same ring.
            out = Image.new(
                "RGBA", (d, d), self._color("HOLIDAY_BORDER"))
            draw = ImageDraw.Draw(out)
            stripe = max(1, round(3 * s))
            for x in range(-d, 2 * d, stripe * 2):
                draw.line(
                    (x, 0, x - d, d),
                    fill=self._color("HOLIDAY_SECONDARY_BORDER"),
                    width=stripe,
                )
            out.putalpha(holiday_ring)
            return out

        holiday_dual_ring = striped_holiday_ring()
        hover = layer(full, self._color("HOVER_BG"))

        def circle_mask() -> "Image.Image":
            # Buttons are round everywhere in the app; the chevrons are
            # buttons, unlike the day cells, so their hover is a circle.
            n = d * ss
            big = Image.new("L", (n, n), 0)
            ImageDraw.Draw(big).ellipse((0, 0, n - 1, n - 1), fill=255)
            return big.resize((d, d), Image.LANCZOS)

        nav_hover = layer(circle_mask(), self._color("HOVER_BG"))

        def chevron(direction: int) -> "Image.Image":
            # Month-nav arrow as geometry, not a font glyph. Tk's
            # compound="center" centres the font LINE box (ascent+descent)
            # -- Segoe UI's baseline sits well below that box's middle,
            # and the angle glyphs' ink hangs around the x-height, so a
            # text chevron rides ~1.5px below the pill's optical centre
            # with no non-magic way to correct it.
            #
            # Construction: PIL's wide-polyline rasteriser is direction-
            # dependent (mirrored polylines yield measurably different
            # ink), so only ONE arm is ever drawn; the lower arm is its
            # exact flip, the opposite direction its exact mirror. The
            # reflection axes pass through the raster centre (n-1)/2 --
            # the masks' centre -- so vertical centring and prev/next
            # parity are structural, not numeric. Proportions follow
            # Fluent chevrons: ~1/3 of the target tall, ~1.5px stroke @
            # 96 DPI, rounded caps; the tip disc doubles as the joint.
            n = d * ss
            c = (n - 1) / 2
            half_h, half_w = 0.16 * n, 0.09 * n
            stroke = max(1.0, 1.5 * s) * ss
            cap = stroke / 2
            back, tip = (c - half_w, c - half_h), (c + half_w, c)
            arm = Image.new("L", (n, n), 0)
            dr = ImageDraw.Draw(arm)
            dr.line([back, tip], fill=255, width=round(stroke))
            for ex, ey in (back, tip):
                dr.ellipse((ex - cap, ey - cap, ex + cap, ey + cap), fill=255)
            glyph = ImageChops.lighter(arm, ImageOps.flip(arm))
            if direction < 0:
                glyph = ImageOps.mirror(glyph)
            return layer(glyph.resize((d, d), Image.LANCZOS),
                         self._color("ACCENT"))

        chev_prev, chev_next = chevron(-1), chevron(+1)

        return {
            "d": d, "layer": layer,
            "masks": {"L": left, "M": Image.new("L", (d, d), 255), "R": right},
            "today": today, "selected": selected, "sel_ring": sel_ring,
            "holiday": holiday, "holiday_secondary": holiday_secondary,
            "holiday_dual": holiday_dual,
            "holiday_primary_ring": holiday_primary_ring,
            "holiday_secondary_ring": holiday_secondary_ring,
            "holiday_dual_ring": holiday_dual_ring,
            "hover": hover, "notch_left": notch_left, "notch_right": notch_right,
            "chev_prev": chev_prev, "chev_next": chev_next, "nav_hover": nav_hover,
            "states": {
                "blank": (), "hover": (hover,),
                "today": (today,),
                "today_holiday_primary": (today, holiday_primary_ring),
                "today_holiday_secondary": (today, holiday_secondary_ring),
                "today_holiday_dual": (today, holiday_dual_ring),
                "selected": (selected, sel_ring),
                "selected_holiday_primary": (selected, holiday_primary_ring),
                "selected_holiday_secondary": (selected, holiday_secondary_ring),
                "selected_holiday_dual": (selected, holiday_dual_ring),
                "holiday_primary": (holiday,),
                "holiday_secondary": (holiday_secondary,),
                "holiday_dual": (holiday_dual,),
            },
        }

    def _build_cell_images(self) -> None:
        """Build the spacer, the rounded state pills, and the band corner-notches.

        Every day and week cell carries one centred image; its width pins the
        column to CELL_PX, so the grid stays uniform and never reflows. The
        state pills -- today, selected, holiday, hover -- are rounded squares
        whose transparent corners let the cell background show through; today
        is a solid accent fill, selected is a soft fill with an accent ring
        (Fluent's filled-vs-outlined today/selected grammar). If either state
        is also a holiday, a saturated purple, teal, or diagonally striped
        outer ring preserves that independent status without adding another
        badge or enlarging the cell. The band corner-notches are the inverse:
        card-coloured cut-outs that round the outer ends of a highlighted week
        while it stays one continuous bg=band bar. The PIL artwork is cached
        per palette (see _cell_art); the Tk images are instance-scoped and
        freed with the popup. Falls back to flat background fills (see
        _refresh) when the artwork cannot be built."""
        self._cell_imgs = {"blank": None}
        try:
            art = self._cell_art(1)
            today = art["today"]
            selected = art["selected"]
            sel_ring = art["sel_ring"]
            holiday = art["holiday"]
            holiday_secondary = art["holiday_secondary"]
            holiday_dual = art["holiday_dual"]
            holiday_primary_ring = art["holiday_primary_ring"]
            holiday_secondary_ring = art["holiday_secondary_ring"]
            holiday_dual_ring = art["holiday_dual_ring"]
            hover = art["hover"]
            notch_left = art["notch_left"]
            notch_right = art["notch_right"]
            chev_prev = art["chev_prev"]
            chev_next = art["chev_next"]
            nav_hover = art["nav_hover"]
            d = art["d"]

            def compose(*layers: "Image.Image") -> "TkPhotoImage":
                out = Image.new("RGBA", (d, d), (0, 0, 0, 0))
                for item in layers:
                    out.alpha_composite(item)
                return ImageTk.PhotoImage(out, master=self.top)

            self._cell_imgs = {
                "blank": ImageTk.PhotoImage(
                    Image.new("RGBA", (d, d), (0, 0, 0, 0)), master=self.top),
                # Width-only spacer for the "CW" header. The week-number
                # column must be exactly as wide as the day cells; otherwise a
                # band-cap image can be centred inside a wider label and the
                # rounded cut-out appears inside the rectangle instead of at
                # the outer edge. Height 1 keeps the header row text-height.
                "cw_header_spacer": ImageTk.PhotoImage(
                    Image.new("RGBA", (d, 1), (0, 0, 0, 0)), master=self.top),
                # *_R variants also carry the card-coloured right corner
                # cut-outs used at the end of an active week band, keeping the
                # week bar rounded even when Sunday itself has a state pill.
                "today": compose(today),
                "today_R": compose(notch_right, today),
                "today_holiday_primary": compose(today, holiday_primary_ring),
                "today_holiday_primary_R": compose(
                    notch_right, today, holiday_primary_ring),
                "today_holiday_secondary": compose(
                    today, holiday_secondary_ring),
                "today_holiday_secondary_R": compose(
                    notch_right, today, holiday_secondary_ring),
                "today_holiday_dual": compose(today, holiday_dual_ring),
                "today_holiday_dual_R": compose(
                    notch_right, today, holiday_dual_ring),
                "selected": compose(selected, sel_ring),
                "selected_R": compose(notch_right, selected, sel_ring),
                "selected_holiday_primary": compose(
                    selected, holiday_primary_ring),
                "selected_holiday_primary_R": compose(
                    notch_right, selected, holiday_primary_ring),
                "selected_holiday_secondary": compose(
                    selected, holiday_secondary_ring),
                "selected_holiday_secondary_R": compose(
                    notch_right, selected, holiday_secondary_ring),
                "selected_holiday_dual": compose(
                    selected, holiday_dual_ring),
                "selected_holiday_dual_R": compose(
                    notch_right, selected, holiday_dual_ring),
                "holiday_primary": compose(holiday),
                "holiday_primary_R": compose(notch_right, holiday),
                "holiday_secondary": compose(holiday_secondary),
                "holiday_secondary_R": compose(notch_right, holiday_secondary),
                "holiday_dual": compose(holiday_dual),
                "holiday_dual_R": compose(notch_right, holiday_dual),
                "hover": compose(hover),
                "hover_R": compose(notch_right, hover),
                "notch_L": compose(notch_left),
                "notch_R": compose(notch_right),
                # Month-navigation chevrons: accent stroke on nothing (rest)
                # or on the shared hover pill.
                "nav_prev": compose(chev_prev),
                "nav_prev_hover": compose(nav_hover, chev_prev),
                "nav_next": compose(chev_next),
                "nav_next_hover": compose(nav_hover, chev_next),
            }
            # Glass: Tk can't draw images on a transparent window, so the same
            # artwork is drawn natively by the backdrop (see _render_overlay).
            if self._glass:
                self._glass_art = self._cell_art(self.GLASS_RATIO)
                self._glass_cache = {}
        except Exception:
            log.exception("Calendar pill images unavailable; using flat fills")
            self._cell_imgs = {"blank": None}

    @classmethod
    def _bind_grid_classes(cls, widget: tk.Misc) -> None:
        """One class binding per event for the whole grid, instead of a Tcl
        callback per cell and event (over 200 per open).

        Tk keeps class bindings on the root until the app quits, so binding a
        popup's own methods on every open kept every closed popup alive. They
        are bound once instead and call whichever popup is open."""
        root = widget.nametowidget(".")
        if cls._bound_root is root:
            return
        cls._bound_root = root
        cls._bindings = []

        def route(method: str):
            def handler(event):
                popup = cls._instance
                return None if popup is None else getattr(popup, method)(event)
            return handler

        def bind(tag: str, sequence: str, method: str) -> None:
            funcid = root.bind_class(tag, sequence, route(method))
            cls._bindings.append((tag, sequence, funcid))

        for sequence in CONTEXT_MENU_EVENTS:
            bind(cls.WEEK_TAG, sequence, "_on_context_menu")
            bind(cls.DAY_TAG, sequence, "_on_context_menu")
        bind(cls.DAY_TAG, "<Enter>", "_on_cell_enter")
        bind(cls.DAY_TAG, "<Leave>", "_on_cell_leave")
        bind(cls.DAY_TAG, "<Button-1>", "_on_day_click")

    #: (tag, sequence, Tcl command) of each class binding, for _unbind_grid_classes.
    _bindings: "list[tuple[str, str, str | None]]" = []

    @classmethod
    def _unbind_grid_classes(cls) -> None:
        """Remove the class bindings and their Tcl commands. They live on the
        root and reference this class, so without this an unloaded calendar
        module would stay in memory for the rest of the run."""
        root, cls._bound_root = cls._bound_root, None
        bindings, cls._bindings = cls._bindings, []
        if root is None:
            return
        for tag, sequence, funcid in bindings:
            try:
                root.unbind_class(tag, sequence)
                if funcid:
                    root.deletecommand(funcid)
            except (tk.TclError, AttributeError):
                pass

    def _build_skeleton(self) -> None:
        self._bind_grid_classes(self.top)
        cell_font = self._font(11)
        self._decor_widgets = []
        self._img_mode = self._cell_imgs["blank"] is not None
        blank = self._cell_imgs["blank"]
        wrapper = tk.Frame(self.card, bg=self._surface())
        wrapper.pack()

        self._titles: list[tuple[tk.Label, tk.Label]] = []
        self._week_cells: list[list[tk.Label]] = []
        self._day_cells: list[list[list[tk.Label]]] = []

        for m in range(self.MONTHS_SHOWN):
            grid_col = 2 * m
            if m:
                tk.Frame(wrapper, bg=self._color("SEPARATOR_BG"), width=1).grid(
                    row=0, column=grid_col - 1, sticky="ns", pady=self._px(10))

            frame = tk.Frame(wrapper, bg=self._surface())
            frame.grid(row=0, column=grid_col, padx=self._px(6))

            # Tk Labels can't mix font weights, so the month (semibold) and
            # year (lighter) are two packed labels in a centred sub-frame.
            title_bar = tk.Frame(frame, bg=self._surface())
            title_bar.grid(row=0, column=0, columnspan=8, sticky="ew",
                           pady=(0, self._px(self.TITLE_WEEKDAY_GAP_PX)))
            # Keep navigation inside the month title rows instead of dedicating
            # permanent outside columns to the chevrons. Those columns were
            # visible as dead strips down the left/right edges of the popup;
            # folding the controls into the title bar lets the day grid use
            # the reclaimed width without changing any navigation behaviour.
            title_bar.grid_columnconfigure(0, minsize=self._px(self.CELL_PX))
            title_bar.grid_columnconfigure(1, weight=1)
            title_bar.grid_columnconfigure(2, minsize=self._px(self.CELL_PX))

            self._make_chevron(title_bar, "\u2039", -1, column=0, visible=(m == 0))

            title_text = tk.Frame(title_bar, bg=self._surface())
            title_text.grid(row=0, column=1)
            month_lbl = tk.Label(title_text, bg=self._surface(), fg=self._color("TITLE_FG"),
                                 font=self._font(14, bold=True))
            month_lbl.pack(side="left")
            year_lbl = tk.Label(title_text, bg=self._surface(), fg=self._color("YEAR_FG"), font=self._font(14))
            year_lbl.pack(side="left", padx=(self._px(4), 0))
            self._titles.append((month_lbl, year_lbl))

            self._make_chevron(
                title_bar, "\u203a", 1, column=2, visible=(m == self.MONTHS_SHOWN - 1))

            for c, name in enumerate(
                    ("CW", "Mo", "Tu", "We", "Th", "Fr", "Sa", "Su")):
                # Day-name headers carry no marker and stay text-height. The
                # CW header is the exception: give it a transparent width-only
                # spacer so column 0 is pinned to the same pixel width as the
                # week-number cap images. If the "CW" text is allowed to make
                # the column wider, the left rounded cap is drawn inside the
                # blue week band and the real outside edge remains square.
                header = tk.Label(
                    frame, text=name, bg=self._surface(),
                    fg=self._color("CW_LABEL_FG") if c == 0 else self._color("DAY_NAME_FG"),
                    font=cell_font, bd=0, highlightthickness=0, padx=0, pady=0)
                if c == 0 and self._img_mode:
                    header.configure(
                        image=self._cell_imgs["cw_header_spacer"],
                        compound="center")
                header.grid(row=1, column=c, padx=0, sticky="nsew")

            week_column: list[tk.Label] = []
            day_grid: list[list[tk.Label]] = []
            for r in range(self.ROWS):
                week_label = tk.Label(frame, bg=self._surface(), fg=self._color("WEEK_NUM_FG"), font=cell_font,
                                      bd=0, highlightthickness=0, padx=0, pady=0)
                week_label.grid(row=2 + r, column=0, padx=0, sticky="nsew")
                week_label.date = None
                week_label.iso_year_week = None
                week_label.bindtags((str(week_label), self.WEEK_TAG) + week_label.bindtags()[1:])
                week_column.append(week_label)
                self._decor_widgets.append(week_label)

                row_cells: list[tk.Label] = []
                for c in range(7):
                    cell = tk.Label(
                        frame, bg=self._surface(), fg=self._color("TITLE_FG"), font=cell_font,
                        bd=0, highlightthickness=0, padx=0, pady=0)
                    if self._img_mode:
                        cell.configure(image=blank, compound="center")
                    else:
                        cell.configure(width=3)
                    cell.grid(row=2 + r, column=c + 1, padx=0, pady=0, sticky="nsew")
                    cell.base_bg = self._surface()
                    cell.hoverable = False
                    cell.date = None
                    cell.bindtags((str(cell), self.DAY_TAG) + cell.bindtags()[1:])
                    row_cells.append(cell)
                    self._decor_widgets.append(cell)
                day_grid.append(row_cells)

            # Pin every week row to a uniform height. Months span 4-6 week
            # rows; without this the empty trailing rows collapse and the
            # popup's height -- anchored to the taskbar edge -- shifts as you
            # navigate. Fixed rows keep every month the same size.
            for r in range(self.ROWS):
                frame.grid_rowconfigure(2 + r, minsize=self._px(self.CELL_PX), uniform="day")

            self._week_cells.append(week_column)
            self._day_cells.append(day_grid)

        tk.Frame(self.card, bg=self._color("SEPARATOR_BG"), height=1).pack(
            fill="x", pady=(self._px(8), self._px(4)))
        # Footer row: one cell per month pane, each centred on it. The panes
        # are the same width, so halving the card puts the Today chip under
        # the left month and the clock under the right one.
        footer_row = tk.Frame(self.card, bg=self._surface())
        footer_row.pack(fill="x")
        for column in range(self.MONTHS_SHOWN):
            footer_row.grid_columnconfigure(column, weight=1, uniform="footer")

        # Clicking today's date jumps back to the current month (same action
        # as the <Home> key); the grid's today cell is only visible when
        # already there, so the affordance lives here too. A chip pill: accent
        # text whose capsule appears on hover, like every other button.
        chip_font = self._font(11, bold=True)
        self._footer = PillButton(footer_row, " ", chip_font, self.scale,
                                  self._go_today, variant="chip", transparent=self._glass,
                                  on_state=lambda state: self._schedule_overlay())
        self._footer.apply_theme(self.theme)
        self._footer.grid(row=0, column=0)

        # Second chip: the selected zone's current UTC offset, styled exactly
        # like Today and opening the time-zone dialog. Present only while the
        # feature is on -- _render_clock grids and ungrids it.
        self._tz_button = PillButton(footer_row, " ", chip_font, self.scale,
                                     self._open_timezone, variant="chip",
                                     transparent=self._glass,
                                     on_state=lambda state: self._schedule_overlay())
        self._tz_button.apply_theme(self.theme)
        self._clock_text = None
        self._render_clock()

    def _make_chevron(
            self, parent: tk.Misc, glyph: str, delta: int, column: int,
            visible: bool = True) -> tk.Label:
        """Month-navigation chevron. In image mode the arrow is geometry
        baked into the pill images (see _build_cell_images), so it is
        ink-centred in the hover pill by construction -- a text glyph over
        the image cannot be, because Tk centres the font line box, not the
        ink. The flat fallback keeps the old text glyph and rectangular
        hover; it inherits the small vertical offset, and only exists when
        ImageTk is missing."""
        chevron = tk.Label(parent, bg=self._surface(), bd=0,
                           highlightthickness=0,
                           cursor="hand2" if visible else "")
        if self._glass:
            # Sized by the spacer; the arrow itself is drawn on the glass.
            key = "nav_prev" if delta < 0 else "nav_next"
            chevron.configure(image=self._cell_imgs["blank"], padx=0, pady=0)
            chevron.decor = (key, None, None) if visible else None
            self._decor_widgets.append(chevron)
        elif self._img_mode:
            key = "nav_prev" if delta < 0 else "nav_next"
            rest_img = self._cell_imgs[key] if visible else self._cell_imgs["blank"]
            hover_img = self._cell_imgs[key + "_hover"]
            chevron.configure(image=rest_img, padx=0, pady=0)
        else:
            chevron.configure(
                text=glyph, font=self._font(14, bold=True),
                fg=self._color("ACCENT") if visible else self._color("CARD_BG"),
                padx=self._px(2), pady=self._px(1))
        chevron.grid(row=0, column=column, sticky="nsew")
        if visible:
            if self._glass:
                def set_hover(hover: bool) -> None:
                    chevron.decor = (key + ("_hover" if hover else ""), None, None)
                    self._schedule_overlay()
                chevron.bind("<Enter>", lambda e: set_hover(True))
                chevron.bind("<Leave>", lambda e: set_hover(False))
            elif self._img_mode:
                chevron.bind("<Enter>", lambda e: chevron.configure(image=hover_img))
                chevron.bind("<Leave>", lambda e: chevron.configure(image=rest_img))
            else:
                chevron.bind("<Enter>", lambda e: chevron.configure(bg=self._color("HOVER_BG")))
                chevron.bind("<Leave>", lambda e: chevron.configure(bg=self._color("CARD_BG")))
            chevron.bind("<Button-1>", lambda e: self._shift_month(delta))
        return chevron

    # -- data fill ----------------------------------------------------------
    def _surface(self) -> str:
        """Tk background for the popup's surfaces: the card colour, or
        transparent over the macOS glass backdrop."""
        return "systemTransparent" if self._glass else self._color("CARD_BG")

    def _cell_bg(self, band: str) -> str:
        # Opaque mode: the row band is the cells' own background, one
        # continuous bar. Glass mode: the band is drawn into the images.
        return self._surface() if self._glass else band

    def _pick(self, state: str, band: str, side: str) -> "TkPhotoImage":
        """The image for a cell in `state` on a row with `band`, at `side`
        (L: week number, M: Monday-Saturday, R: Sunday) of the row.

        Opaque mode reuses the prebuilt images: on a highlighted row the
        band's outer ends are rounded by card-coloured corner notches (the
        "notch_*" and "*_R" variants). Glass mode has no card colour to notch
        with, so the band segment is composited under the state and cached."""
        if self._glass:
            # Only sizes the label: the artwork is the cell's `decor`, drawn
            # natively on the glass by _render_overlay.
            return self._cell_imgs["blank"]
        if band == self._color("CARD_BG"):
            return self._cell_imgs[state]
        if side == "L":
            return self._cell_imgs["notch_L" if state == "blank" else state]
        if side == "R":
            return self._cell_imgs["notch_R" if state == "blank" else state + "_R"]
        return self._cell_imgs[state]

    def _decor(self, state: str, band: str, side: str) -> "tuple | None":
        """Glass-mode artwork key for a cell: its state on its band segment,
        or None when there is nothing to draw."""
        banded = band != self._color("CARD_BG")
        if state == "blank" and not banded:
            return None
        return (state, band if banded else None, side)

    def _glass_image(self, key: tuple) -> "Image.Image":
        """The ratio-2 artwork for a decor key, composited once and cached."""
        image = self._glass_cache.get(key)
        if image is None:
            art = self._glass_art
            state, band, side = key
            if state.startswith("nav_"):  # chevrons: ("nav_prev[_hover]", None, None)
                direction = "chev_prev" if state.startswith("nav_prev") else "chev_next"
                layers = ([art["nav_hover"]] if state.endswith("_hover") else []) + [art[direction]]
            else:
                layers = ([art["layer"](art["masks"][side], band)] if band else [])
                layers += list(art["states"][state])
            image = Image.new("RGBA", (art["d"], art["d"]), (0, 0, 0, 0))
            for item in layers:
                image.alpha_composite(item)
            self._glass_cache[key] = image
        return image

    def _schedule_overlay(self) -> None:
        if self._glass and not self._overlay_pending:
            self._overlay_pending = True
            self.top.after_idle(self._render_overlay)

    def _render_overlay(self) -> None:
        """Composite every cell, chevron and chip decoration at its on-screen
        position into one Retina image and hand it to the glass backdrop."""
        self._overlay_pending = False
        backdrop = self._backdrop
        if backdrop is None:
            return
        try:
            top = self.top
            ox, oy = top.winfo_rootx(), top.winfo_rooty()
            width, height = top.winfo_width(), top.winfo_height()
        except tk.TclError:
            return
        k = self.GLASS_RATIO
        canvas = Image.new("RGBA", (width * k, height * k), (0, 0, 0, 0))
        size = self._glass_art["d"]
        for widget in self._decor_widgets:
            key = getattr(widget, "decor", None)
            if key is None or not widget.winfo_ismapped():
                continue
            x = (widget.winfo_rootx() - ox) * k + (widget.winfo_width() * k - size) // 2
            y = (widget.winfo_rooty() - oy) * k + (widget.winfo_height() * k - size) // 2
            canvas.alpha_composite(self._glass_image(key), (max(0, x), max(0, y)))
        draw = ImageDraw.Draw(canvas)
        for chip in (self._footer, self._tz_button):
            fill = chip.capsule_fill(chip.visual_state)
            if fill is None or not chip.winfo_ismapped():
                continue
            x0, y0 = (chip.winfo_rootx() - ox) * k, (chip.winfo_rooty() - oy) * k
            w, h = chip.winfo_width() * k, chip.winfo_height() * k
            draw.rounded_rectangle((x0, y0, x0 + w - 1, y0 + h - 1), radius=h / 2, fill=fill)
        backdrop.set_overlay(canvas, width, height)

    def _refresh(self) -> None:
        today = dt.date.today()
        # Compare (ISO year, week): the week number alone collides across
        # year boundaries and would highlight the wrong week in Dec/Jan.
        today_iso = today.isocalendar()[:2]
        sel = self.selected_date
        sel_iso = sel.isocalendar()[:2] if sel else None

        cell_font = self._font(11)
        primary_country = self.settings.holiday_country_primary
        secondary_country = self.settings.holiday_country_secondary
        # Only the years on screen are kept: one, or two in December.
        hold_holidays((primary_country, secondary_country),
                      {self.display_month.year + (self.display_month.month + m - 1) // 12
                       for m in range(self.MONTHS_SHOWN)})

        # Manual day join: strftime %d zero-pads and the unpadded specifier
        # is platform-specific (%#d Windows, %-d POSIX).
        self._footer.set_text("Today \u00b7 {:02d}-{:02d}-{}".format(
            today.day, today.month, today.year))

        for m in range(self.MONTHS_SHOWN):
            year = self.display_month.year + (self.display_month.month + m - 1) // 12
            month = (self.display_month.month + m - 1) % 12 + 1
            first = dt.date(year, month, 1)
            next_first = dt.date(year + 1, 1, 1) if month == 12 else dt.date(year, month + 1, 1)
            last = next_first - dt.timedelta(days=1)
            days_in_month = (next_first - first).days

            month_lbl, year_lbl = self._titles[m]
            month_lbl.configure(text=first.strftime("%B"))
            year_lbl.configure(text=str(year))

            # A week band belongs to the month pane that contains the actual
            # anchor date. ISO weeks often span two visible months (e.g. Jun 29
            # and Jul 1 are both CW27); highlighting every matching ISO week in
            # every pane makes the selection/current-week appear duplicated.
            # Restricting by pane month gives exactly one current band and
            # exactly one selected band.
            pane_is_today_month = (year, month) == (today.year, today.month)
            pane_is_selected_month = (
                sel is not None and (year, month) == (sel.year, sel.month)
            )

            grid_start = first - dt.timedelta(days=first.weekday())
            row_bands: list[tuple[tuple[int, int] | None, str]] = []

            for r in range(self.ROWS):
                row_start = grid_start + dt.timedelta(days=7 * r)
                row_end = row_start + dt.timedelta(days=6)
                row_has_month_day = row_start <= last and row_end >= first
                row_iso = row_start.isocalendar()[:2] if row_has_month_day else None

                if row_iso is not None and pane_is_today_month and row_iso == today_iso:
                    band = self._color("CURRENT_WEEK_BG")
                elif row_iso is not None and pane_is_selected_month and row_iso == sel_iso:
                    band = self._color("SELECTED_WEEK_BG")
                else:
                    band = self._color("CARD_BG")

                row_bands.append((row_iso, band))

                week_label = self._week_cells[m][r]
                week_label.decor = self._decor("blank", band, "L")
                week_label.date = None
                week_label.iso_year_week = row_iso
                week_text = str(row_iso[1]) if row_iso is not None else ""
                if self._img_mode:
                    # Column 0 is pinned to the same pixel width as the cap
                    # image by the CW header spacer. Draw the left notch across
                    # the full week-number cell and keep the number centred.
                    week_label.configure(
                        text=week_text,
                        bg=self._cell_bg(band),
                        fg=self._color("WEEK_NUM_FG"),
                        font=cell_font,
                        image=self._pick("blank", band, "L"),
                        compound="center",
                        anchor="center",
                    )
                else:
                    week_label.configure(text=week_text, bg=band, fg=self._color("WEEK_NUM_FG"), font=cell_font)

                for c, cell in enumerate(self._day_cells[m][r]):
                    side = "R" if c == 6 else "M"
                    cell.base_bg = self._cell_bg(band)
                    cell.hoverable = False
                    cell.date = None
                    cell.iso_year_week = None
                    cell.holiday_name = None
                    cell.configure(
                        text="", bg=self._cell_bg(band), fg=self._color("TITLE_FG"),
                        font=cell_font, highlightthickness=0)
                    if self._img_mode:
                        rest_img = self._pick("blank", band, side)
                        cell.configure(image=rest_img)
                        cell.rest_img = rest_img
                        cell.hover_img = self._pick("hover", band, side)
                        cell.decor = cell.rest_decor = self._decor("blank", band, side)
                        cell.hover_decor = self._decor("hover", band, side)

            row, col = 0, first.weekday()  # Monday == 0
            for day in range(1, days_in_month + 1):
                date = first.replace(day=day)
                iso_year, iso_week, _ = date.isocalendar()
                row_iso = (iso_year, iso_week)
                band = row_bands[row][1]

                is_today = date == today
                is_selected = sel is not None and date == sel and not is_today
                primary_holidays = holiday_names(primary_country, date)
                secondary_holidays = holiday_names(secondary_country, date)
                has_primary = bool(primary_holidays)
                has_secondary = bool(secondary_holidays)
                tooltip_lines: list[str] = []
                if has_primary:
                    tooltip_lines.append(
                        f"{holiday_country_name(primary_country) or primary_country}: "
                        + "; ".join(primary_holidays))
                if has_secondary:
                    tooltip_lines.append(
                        f"{holiday_country_name(secondary_country) or secondary_country}: "
                        + "; ".join(secondary_holidays))
                holiday_tooltip = "\n".join(tooltip_lines) or None

                cell = self._day_cells[m][row][col]
                fg, hoverable = self._color("TITLE_FG"), True
                if has_primary and has_secondary:
                    holiday_kind = "dual"
                elif has_primary:
                    holiday_kind = "primary"
                elif has_secondary:
                    holiday_kind = "secondary"
                else:
                    holiday_kind = None

                if self._img_mode:
                    # Cell background is the continuous row band. Marker images
                    # are layered over it; on Sunday, *_R variants also paint
                    # card-coloured cut-outs into the band's outer right corners.
                    #
                    # Today and selection remain the dominant state. A holiday
                    # adds an independent saturated outer ring, so neither state
                    # can hide the holiday. Dual-country holidays use the same
                    # purple/teal diagonal treatment as normal dual markers.
                    side = "R" if col == 6 else "M"
                    if is_today:
                        state = (
                            f"today_holiday_{holiday_kind}"
                            if holiday_kind else "today"
                        )
                        rest, fg, hoverable = (
                            state, self._color("TODAY_FG"), False)
                    elif is_selected:
                        state = (
                            f"selected_holiday_{holiday_kind}"
                            if holiday_kind else "selected"
                        )
                        rest = state
                        # Keep holiday indication visible rather than replacing
                        # it with the generic hover pill.
                        hoverable = holiday_kind is None
                    elif holiday_kind:
                        rest, hoverable = "holiday_" + holiday_kind, False
                        fg = self._color("HOLIDAY_FG")
                    else:
                        rest = "blank"

                    rest_img = self._pick(rest, band, side)
                    cell.configure(text=str(day), bg=self._cell_bg(band), fg=fg,
                                   image=rest_img, font=cell_font)
                    cell.rest_img = rest_img
                    cell.hover_img = self._pick("hover", band, side)
                    cell.decor = cell.rest_decor = self._decor(rest, band, side)
                    cell.hover_decor = self._decor("hover", band, side)
                    cell.base_bg = self._cell_bg(band)
                else:
                    # Rare Pillow-without-ImageTk fallback. Tk cannot render a
                    # striped outline, but a solid holiday-coloured border still
                    # prevents Today/selection from hiding the holiday state.
                    bg = band
                    border = None
                    if is_today:
                        bg, fg, hoverable = (
                            self._color("ACCENT"), self._color("TODAY_FG"), False)
                        if holiday_kind:
                            border = self._color(
                                "HOLIDAY_SECONDARY_BORDER"
                                if holiday_kind == "secondary"
                                else "HOLIDAY_BORDER")
                    elif is_selected:
                        bg = self._color("SELECTED_DAY_FILL")
                        if holiday_kind:
                            border = self._color(
                                "HOLIDAY_SECONDARY_BORDER"
                                if holiday_kind == "secondary"
                                else "HOLIDAY_BORDER")
                            hoverable = False
                    elif holiday_kind:
                        bg = (self._color("HOLIDAY_BG") if has_primary
                              else self._color("HOLIDAY_SECONDARY_BG"))
                        fg, hoverable = self._color("HOLIDAY_FG"), False
                    cell.configure(
                        text=str(day), bg=bg, fg=fg, font=cell_font,
                        highlightbackground=border or bg,
                        highlightthickness=self._px(2) if border else 0,
                    )
                    cell.base_bg = bg
                cell.hoverable = hoverable
                cell.date = date
                cell.iso_year_week = row_iso
                cell.holiday_name = holiday_tooltip

                col += 1
                if col == 7:
                    col = 0
                    row += 1
        self._schedule_overlay()

    # -- interaction --------------------------------------------------------
    def _copy_to_clipboard(self, value: str) -> None:
        try:
            self.top.clipboard_clear()
            self.top.clipboard_append(value)
            self.top.update_idletasks()
        except Exception:
            log.exception("Failed to copy calendar value to clipboard")

    def _on_context_menu(self, event) -> None:
        widget = event.widget
        on_date = getattr(widget, "date", None)
        iso_year_week = getattr(widget, "iso_year_week", None)
        if on_date is None and iso_year_week is None:
            return

        menu = tk.Menu(
            self.top,
            tearoff=False,
            bg=self._color("MENU_BG"),
            fg=self._color("MENU_FG"),
            activebackground=self._color("MENU_ACTIVE_BG"),
            activeforeground=self._color("MENU_ACTIVE_FG"),
            bd=0,
            relief="flat",
        )
        if on_date is not None:
            menu.add_command(
                label=f"Copy date ({format_gui_date(on_date)})",
                command=lambda d=on_date: self._copy_to_clipboard(format_gui_date(d)),
            )
            menu.add_command(
                label=f"Copy week ({format_calendar_week(on_date)})",
                command=lambda d=on_date: self._copy_to_clipboard(format_calendar_week(d)),
            )
        elif iso_year_week is not None:
            menu.add_command(
                label=f"Copy week ({format_iso_week(*iso_year_week)})",
                command=lambda yw=iso_year_week: self._copy_to_clipboard(format_iso_week(*yw)),
            )

        self._hide_tip()
        self._context_menu_visible = True
        try:
            menu.tk_popup(event.x_root, event.y_root)
        finally:
            menu.grab_release()
            self._context_menu_visible = False
            try:
                self.top.focus_force()
            except tk.TclError:
                pass

    def _on_cell_enter(self, event) -> None:
        widget = event.widget
        if getattr(widget, "hoverable", False):
            if self._glass:
                widget.decor = getattr(widget, "hover_decor", None)
                self._schedule_overlay()
            elif self._img_mode:
                widget.configure(image=getattr(widget, "hover_img", self._cell_imgs["hover"]))
            else:
                widget.configure(bg=self._color("HOVER_BG"))
        name = getattr(widget, "holiday_name", None)
        if name:
            self._schedule_tip(widget, name)

    def _on_cell_leave(self, event) -> None:
        widget = event.widget
        if getattr(widget, "hoverable", False):
            if self._glass:
                widget.decor = getattr(widget, "rest_decor", None)
                self._schedule_overlay()
            elif self._img_mode:
                widget.configure(
                    image=getattr(widget, "rest_img", self._cell_imgs["blank"]))
            else:
                widget.configure(bg=widget.base_bg)
        self._hide_tip()

    # -- holiday tooltip ----------------------------------------------------
    def _schedule_tip(self, widget: tk.Widget, text: str) -> None:
        self._cancel_tip_timer()
        self._tip_after = self.top.after(
            self.TOOLTIP_DELAY_MS, lambda: self._show_tip(widget, text))

    def _show_tip(self, widget: tk.Widget, text: str) -> None:
        # A borderless child window, never focused, so it can't trip the
        # popup's focus-out / foreground close (also guarded by _tip_visible).
        self._hide_tip()
        try:
            tip = tk.Toplevel(self.top)
            tip.overrideredirect(True)
            tip.attributes("-topmost", True)
            glass = self._glass
            if glass:
                tip.wm_attributes("-transparent", True)
                tip.configure(bg="systemTransparent")
            border = tk.Frame(tip, bg="systemTransparent" if glass
                              else self._color("TOOLTIP_BORDER"))
            border.pack()
            label = tk.Label(border, text=text, fg=self._color("TITLE_FG"),
                             bg="systemTransparent" if glass else self._color("TOOLTIP_BG"),
                             font=self._font(11), padx=self._px(8), pady=self._px(4))
            label.pack(padx=1, pady=1)
            tip.update_idletasks()
            tw, th = tip.winfo_reqwidth(), tip.winfo_reqheight()
            w = self._work
            x = max(w.left, min(widget.winfo_rootx(), w.right - tw))
            y = widget.winfo_rooty() + widget.winfo_height() + self._px(3)
            if y + th > w.bottom:  # no room below the cell -> sit above it
                y = widget.winfo_rooty() - th - self._px(3)
            y = max(w.top, y)
            tip.geometry(f"+{int(x)}+{int(y)}")
            set_rounded_corners(toplevel_hwnd(tip), small=True)
            self._tip = tip
            self._tip_visible = True
            if glass:
                tip.update_idletasks()
                # Concentric with the text's padding box: the vertical pad
                # plus half the line, so a one-line tip reads as a capsule.
                radius = min(tip.winfo_reqheight() / 2.0, self._px(10))
                self._tip_backdrop = attach_glass_backdrop(
                    tip, radius, self.settings.is_dark_mode)
        except tk.TclError:
            self._tip = None
            self._tip_visible = False

    def _cancel_tip_timer(self) -> None:
        if self._tip_after is not None:
            try:
                self.top.after_cancel(self._tip_after)
            except Exception:
                pass
            self._tip_after = None

    def _hide_tip(self) -> None:
        self._cancel_tip_timer()
        if self._tip_backdrop is not None:
            self._tip_backdrop.close()
            self._tip_backdrop = None
        if self._tip is not None:
            try:
                self._tip.destroy()
            except tk.TclError:
                pass
            self._tip = None
        self._tip_visible = False

    # -- time-zone chip -----------------------------------------------------
    def _render_clock(self) -> None:
        """Show or hide the offset chip and keep its text current.

        Reads the settings on every tick, so a change made in the time-zone
        dialog lands here without any wiring between the two windows. With the
        feature off the chip is ungridded and Today spans the whole footer, so
        it returns to the centre of the popup rather than the centre of the
        left month.
        """
        text = ""
        if self.settings.timezone_enabled and self._on_timezone is not None:
            if not self._zones_held:
                # Loaded with the clock, not at startup: most people never
                # turn it on. Checks the saved zone on the way (use_zones).
                self._zones_held = True
                use_clock(self, self.settings)
            key = resolve_zone_key(self.settings.timezone_key)
            reading = zone_now_and_offset(key) if key else None
            if reading is not None:
                now, offset = reading
                text = f"{format_offset_short(offset)} \u00b7 {now:%H:%M}"
        if text == self._clock_text:
            return
        self._clock_text = text
        try:
            if text:
                self._footer.grid_configure(column=0, columnspan=1)
                self._tz_button.set_text(text)
                self._tz_button.grid(row=0, column=self.MONTHS_SHOWN - 1)
            else:
                self._tz_button.grid_forget()
                self._footer.grid_configure(column=0, columnspan=self.MONTHS_SHOWN)
        except tk.TclError:
            pass

    def _open_timezone(self) -> None:
        if self._on_timezone is None:
            return
        try:
            self._on_timezone()
        except Exception:
            log.exception("Could not open the time-zone dialog")

    def _tick_clock(self) -> None:
        self._clock_after = None
        try:
            if not self.top.winfo_exists():
                return
            self._render_clock()
            self._clock_after = self.top.after(CLOCK_TICK_MS, self._tick_clock)
        except tk.TclError:
            self._clock_after = None

    def _cancel_clock_timer(self) -> None:
        if self._clock_after is not None:
            try:
                self.top.after_cancel(self._clock_after)
            except Exception:
                pass
            self._clock_after = None

    def _on_day_click(self, event) -> None:
        date = getattr(event.widget, "date", None)
        if date is None:  # blank lead-in/trail-out cell
            return
        self.selected_date = date
        self._refresh()

    def _on_mousewheel(self, event) -> None:
        self._shift_month(-1 if event.delta > 0 else 1)

    def _shift_month(self, delta: int) -> None:
        absolute_month = self.display_month.year * 12 + self.display_month.month - 1 + delta
        candidate = dt.date(absolute_month // 12, absolute_month % 12 + 1, 1)
        bounded = min(self.MAX_MONTH, max(self.MIN_MONTH, candidate))
        if bounded != self.display_month:
            self.display_month = bounded
            self._refresh()

    def _go_today(self) -> None:
        today_month = dt.date.today().replace(day=1)
        self.display_month = min(self.MAX_MONTH, max(self.MIN_MONTH, today_month))
        self._refresh()

    # -- placement & lifetime -----------------------------------------------
    def _place(self, cursor: POINT, monitor: RECT, work: RECT) -> None:
        self.top.update_idletasks()
        width = self.top.winfo_reqwidth()
        height = self.top.winfo_reqheight()
        margin = self._px(8)

        edge = tray_edge(monitor, work)

        if edge in ("bottom", "top"):
            x = cursor.x - width // 2
            y = work.bottom - height - margin if edge == "bottom" else work.top + margin
        else:
            y = cursor.y - height // 2
            x = work.left + margin if edge == "left" else work.right - width - margin

        x = max(work.left + margin, min(x, work.right - width - margin))
        y = max(work.top + margin, min(y, work.bottom - height - margin))

        self.top.geometry(f"{width}x{height}+{x}+{y}")

    def _watch_foreground(self) -> None:
        """Close when another window takes the foreground -- the behaviour of
        the native clock flyout. Covers the cases <FocusOut> misses."""
        try:
            if not self.top.winfo_exists():
                return
        except tk.TclError:
            return
        if owns_foreground(self._hwnd):
            self._had_focus = True
        elif self._had_focus and not self._tip_visible:
            self._dismiss()
            return
        self.top.after(150, self._watch_foreground)

    def _dismiss(self) -> None:
        """Close because the flyout lost the focus (see toggle)."""
        CalendarPopup._dismissed_at = time.monotonic()
        self.close()

    def close(self) -> None:
        if CalendarPopup._instance is self:
            CalendarPopup._instance = None
        self._cancel_clock_timer()
        self._hide_tip()
        if self._backdrop is not None:
            self._backdrop.close()
            self._backdrop = None
        # Nothing of the calendar stays in memory while it is closed: the
        # holidays, zones and artwork are a few ms to load again on the next
        # open, and the module itself is unloaded once no window is open.
        release_holidays()
        if self._zones_held:
            self._zones_held = False
            drop_zones(self)
        CalendarPopup._art_cache.clear()
        release_memory_soon(self.top)
        try:
            self.top.destroy()
        except tk.TclError:
            pass


def __weeknumber_unload__() -> None:
    """Called by weeknumber_core.ModuleSweeper before this module is
    unloaded: the grid's class bindings are the only thing it leaves on the
    Tk root."""
    CalendarPopup._unbind_grid_classes()
