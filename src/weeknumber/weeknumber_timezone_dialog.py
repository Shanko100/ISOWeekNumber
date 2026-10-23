"""Time-zone picker dialog for WeekNumber.

A single, centred window: a switch that turns the calendar clock on, a world
map coloured by standard UTC offset, and a standard read-only dropdown. Map and dropdown drive each other -- clicking a region selects the
listed city inside it, choosing from the dropdown moves the highlight.

The map is a filled choropleth rather than an outline drawing: every band
arrives pre-clipped into a sea part and a land part, so each zone can be washed
over open water and saturated over land the way a printed time-zone map is.
Colours cycle through five hues by band position, which guarantees neighbouring
zones differ even where two of them are only half an hour apart.

The switch and the buttons are the app's hand-drawn controls (see
weeknumber_widgets): Tk's native ones neither scale cleanly nor take a dark
palette. Nothing on screen leads into Timezone Dominator: see is_egg_point.

Unlike the calendar flyout this is a real titled window: it must survive the
focus changes that come with operating a dropdown, so it deliberately does not
close on focus loss.
"""

from __future__ import annotations

import tkinter as tk
from tkinter import ttk

from PIL import Image, ImageDraw, ImageTk
from typing import TYPE_CHECKING

from weeknumber_dialog import TitledDialog
from weeknumber_map import WorldMapRenderer, fit_map_size
from weeknumber_widgets import (
    PillButton as _PillButton,
    Switch as _Switch,
    blend as _blend,
    style_combobox,
)
from weeknumber_core import CLOCK_TICK_MS, SettingsStore, log
from weeknumber_timezone import (
    ZONE_BANDS,
    ZONES,
    TimeZoneEntry,
    band_at,
    drop_map,
    find_zone,
    format_offset,
    nearest_zone,
    resolve_zone_key,
    system_zone_key,
    use_map,
    use_zones,
    zone_now_and_offset,
)

if TYPE_CHECKING:
    from collections.abc import Callable

#: The way into Timezone Dominator: three quick clicks on Afghanistan, the one country
#: whose clocks run on UTC+4:30, so its time-zone band is exactly its outline.
EGG_OFFSET_HOURS = 4.5
EGG_CLICKS = 3
EGG_WINDOW_MS = 900


def is_egg_point(lon: float, lat: float) -> bool:
    band = band_at(lon, lat)
    return 0 <= band < len(ZONE_BANDS) and float(ZONE_BANDS[band].utc) == EGG_OFFSET_HOURS


class ClickCounter:
    """Counts hits that land within `window_ms` of the first one; `hit()`
    is True on the `count`-th. Times are Tk event times in milliseconds."""

    def __init__(self, count: int = EGG_CLICKS, window_ms: int = EGG_WINDOW_MS) -> None:
        self.count = count
        self.window_ms = window_ms
        self._times: list[int] = []

    @property
    def started(self) -> bool:
        return bool(self._times)

    def reset(self) -> None:
        self._times.clear()

    def hit(self, time_ms: int) -> bool:
        if self._times and not 0 <= time_ms - self._times[0] <= self.window_ms:
            self._times.clear()
        self._times.append(time_ms)
        if len(self._times) < self.count:
            return False
        self._times.clear()
        return True


class TimeZoneDialog(TitledDialog):
    """Single-instance time-zone picker."""

    TITLE = "Time Zone"
    #: Preferred map width in px @ 96 DPI; shrunk to fit the work area.
    MAP_WIDTH_PX = 860
    #: Height of the offset ruler above the map, in px @ 96 DPI.
    RULER_PX = 18
    MIN_MAP_WIDTH_PX = 300

    def __init__(self, master: tk.Misc, settings: "SettingsStore",
                 on_change: "Callable[[], None] | None" = None,
                 on_play: "Callable[[TimeZoneDialog], None] | None" = None) -> None:
        work = self._init_window(master, settings)
        use_zones(self, settings)  # the zone list, checking the saved zone
        use_map(self)  # the bands and borders, while this window is open
        self._on_change = on_change
        #: The Timezone Dominator easter egg: opens the lobby over this window, the backdrop.
        #: There is no button: three quick clicks on Afghanistan start it.
        self._on_play = on_play
        self._egg = ClickCounter()
        self._egg_before: "str | None" = None  # the zone to restore after it
        self._transforming = False   # a crossfade is running
        self._board = None           # the Timezone Dominator board while the map shows it
        self._board_image = None
        self._sheet: "tk.Toplevel | None" = None  # the lobby, kept above
        self._map_image = None
        self._clock_after: "str | None" = None
        self._zones: list[TimeZoneEntry] = list(ZONES)
        # The saved zone, or the machine's own when nothing valid is saved. It
        # may legitimately not be one of the listed cities -- the OS knows far
        # more zones than the picker lists -- in which case the map shows no
        # highlight but the clock still works.
        self._selected_key: "str | None" = resolve_zone_key(settings.timezone_key)

        # Real values land in _fit_map(); these keep drawing total in the
        # unlikely event it is requested before the first sizing pass.
        self._base_map = None
        self._map_photo = None
        self._map_width = self._px(10)
        self._map_height = self._px(10)
        self._ruler_height = self._px(self.RULER_PX)
        self._renderer = WorldMapRenderer(self.theme)
        self._build()
        self._fit_map(work)
        self._apply_theme()
        self._center(work)

        self.top.bind("<FocusIn>", self._on_focus_in, add="+")
        self.top.deiconify()
        self._raise()
        self._tick_clock()

    # -- construction -------------------------------------------------------
    def _build(self) -> None:
        pad = self._px(16)
        self.card = tk.Frame(self.top, padx=pad, pady=pad)
        self.card.pack(fill="both", expand=True)

        self.switch_row = tk.Frame(self.card)
        self.switch_row.pack(fill="x", pady=(0, self._px(12)))
        self.switch = _Switch(self.switch_row, self.scale,
                              self.settings.timezone_enabled,
                              self._on_toggle_enabled)
        self.switch.pack(side="left")
        self.switch_label = tk.Label(self.switch_row, font=self._font(11),
                                     text="Show a time zone clock in the calendar",
                                     cursor="hand2")
        self.switch_label.pack(side="left", padx=(self._px(10), 0))
        self.switch_label.bind("<Button-1>", lambda e: self.switch.toggle())

        # 1px hairline around the map, matching the calendar's surface stroke.
        self.map_border = tk.Frame(self.card)
        self.map_border.pack()
        self.canvas = tk.Canvas(self.map_border, width=self._px(10),
                                height=self._px(10), highlightthickness=0, bd=0,
                                cursor="hand2")
        self.canvas.pack(padx=1, pady=1)
        self.canvas.bind("<Button-1>", self._on_map_click)

        self.combo_row = tk.Frame(self.card)
        self.combo_row.pack(fill="x", pady=(self._px(14), 0))
        self.combo_label = tk.Label(self.combo_row, text="Time zone",
                                    font=self._font(11))
        self.combo_label.pack(side="left", padx=(0, self._px(10)))
        self.combo_var = tk.StringVar()
        self.combo = ttk.Combobox(
            self.combo_row, textvariable=self.combo_var, state="readonly",
            style="WeekNumber.TCombobox", font=self._font(11),
            height=18, values=self._combo_values())
        self.combo.pack(side="left", fill="x", expand=True)
        self.combo.bind("<<ComboboxSelected>>", self._on_combo_selected)

        self.footer_row = tk.Frame(self.card)
        self.footer_row.pack(fill="x", pady=(self._px(14), 0))
        self.status = tk.Label(self.footer_row, font=self._font(11), anchor="w")
        self.status.pack(side="left")
        self.close_button = _PillButton(self.footer_row, "Close",
                                        self._font(11, bold=True), self.scale,
                                        self.close)
        self.close_button.pack(side="right")

        if not self._zones:
            self.combo.configure(state="disabled")
            self.canvas.configure(cursor="")
        self._sync_combo_selection()

    def _fit_map(self, work) -> None:
        """Size the map to the largest that still fits this monitor.

        The chrome is measured rather than assumed: at 150% or 200% scaling the
        controls grow too, and a hard-coded reserve would either waste space or
        push the dialog off-screen.
        """
        self.top.update_idletasks()
        placeholder = self._px(10)
        chrome_height = max(0, self.top.winfo_reqheight() - placeholder)
        chrome_width = max(0, self.top.winfo_reqwidth() - placeholder)
        margin = self._px(self.SCREEN_MARGIN_PX)

        available_width = (work.right - work.left) - margin - chrome_width
        available_height = (work.bottom - work.top) - margin - chrome_height
        self._map_width, self._map_height = fit_map_size(
            self._px(self.MAP_WIDTH_PX), self._px(self.MIN_MAP_WIDTH_PX),
            available_width, available_height)
        self._renderer.resize(self._map_width, self._map_height)
        self._ruler_height = self._px(self.RULER_PX)
        self.canvas.configure(width=self._map_width,
                              height=self._map_height + self._ruler_height)

    def _combo_values(self) -> list[str]:
        """Dropdown rows: current offset first, then the cities.

        Offsets are read once per dialog; a DST transition while the window is
        open would leave a row stale until it is reopened, which is a fair
        trade for not re-querying sixty zones on every clock tick.
        """
        values = []
        for zone in self._zones:
            reading = zone_now_and_offset(zone.key)
            values.append(f"({format_offset(reading[1] if reading else None)})"
                          f"  {zone.label}")
        return values

    # -- theming ------------------------------------------------------------
    def _apply_theme(self) -> None:
        """Repaint every widget from the active palette.

        Reconfiguring beats rebuilding: the widget set is fixed and small, and
        a rebuild would drop the user's in-progress selection.
        """
        super()._apply_theme()
        self._renderer.theme = self.theme
        card_bg = self._color("CARD_BG")
        self.top.configure(bg=card_bg)
        for frame in (self.card, self.switch_row, self.combo_row, self.footer_row):
            frame.configure(bg=card_bg)
        self.switch_label.configure(bg=card_bg, fg=self._color("TITLE_FG"))
        self.combo_label.configure(bg=card_bg, fg=self._color("TITLE_FG"))
        self.status.configure(bg=card_bg, fg=self._color("YEAR_FG"))
        self.map_border.configure(bg=self._color("CARD_BORDER"))
        self.canvas.configure(bg=card_bg)
        self.switch.apply_theme(self.theme)
        self.close_button.apply_theme(self.theme)

        try:
            style_combobox(self.top, "WeekNumber.TCombobox", self.theme, self.scale)
        except tk.TclError:
            log.exception("Could not style the time-zone dropdown")
        self._draw_map()
        if self._board is not None and not self._transforming:
            self._show_board()
        self._render_status()

    # -- map rendering ------------------------------------------------------
    # Projection, oversampling and the base layers live in WorldMapRenderer,
    # shared with the Timezone Dominator board so the two maps look identical.

    def _render_base(self) -> "Image.Image":
        """Every band's sea first, then every band's land: land must never be
        painted over by a neighbour's sea polygon. Borders go on top."""
        image, draw = self._renderer.new_image()
        self._renderer.paint_band_seas(draw)
        self._renderer.paint_band_land(draw)
        self._renderer.paint_borders(draw)
        return image

    def _compose_map(self) -> "ImageTk.PhotoImage":
        """Base map plus the current selection, resampled to display size."""
        renderer = self._renderer
        scale = renderer.supersample
        image = self._base_map.copy()
        draw = ImageDraw.Draw(image)
        zone = find_zone(self._selected_key)
        if zone is not None:
            edge = self._color("MAP_SELECT_LAND")
            select_sea = _blend(edge, self._color("MAP_OCEAN"), WorldMapRenderer.SEA_MIX)
            stroke = max(1, round(scale))
            if 0 <= zone.band < len(ZONE_BANDS):
                band = ZONE_BANDS[zone.band]
                renderer.paint_rings(draw, band.sea, select_sea, outline=edge,
                                     width=stroke)
                renderer.paint_rings(draw, band.land, edge)
            x, y = renderer.hi_poly([zone.lon, zone.lat])[0]
            radius = max(3.0, self._map_width * scale * 0.0065)
            draw.ellipse((x - radius, y - radius, x + radius, y + radius),
                         fill=edge, outline=self._color("TODAY_FG"),
                         width=max(1, round(scale * 1.5)))
        self._map_image = renderer.downsample(image)  # kept for the Timezone Dominator crossfade
        return ImageTk.PhotoImage(self._map_image)

    def _draw_map(self) -> None:
        """Rebuild the whole map. Called once per theme or resize, not per
        selection: changing zone only recomposes the cached base bitmap."""
        self.canvas.delete("all")
        self._base_map = self._render_base()
        self._map_photo = self._compose_map()
        self.canvas.create_image(0, self._ruler_height, anchor="nw",
                                 image=self._map_photo, tags="map")
        self.canvas.create_rectangle(
            0, self._ruler_height, self._map_width - 1,
            self._ruler_height + self._map_height - 1,
            outline=self._color("MAP_EDGE"), width=1, tags="frame")
        self._draw_ruler()

    def _draw_ruler(self) -> None:
        """The offset scale along the top, one cell per whole hour.

        Cells sit on the nominal 15-degree slots and take the tint of the band
        that actually keeps that offset, so ruler and map read as one legend:
        find the number, look straight down. Kept as canvas items rather than
        baked into the bitmap so the numerals stay text-rendered and crisp.
        """
        tint_for_offset = {int(band.utc): band.tint
                           for band in ZONE_BANDS if float(band.utc).is_integer()}
        half_w = self._map_width / 2.0
        font = (self._family, -max(7, self._px(9)))
        for hour in range(-12, 13):
            west = max(-180.0, hour * 15.0 - 7.5)
            east = min(180.0, hour * 15.0 + 7.5)
            x0 = half_w + (west / 180.0) * half_w
            x1 = half_w + (east / 180.0) * half_w
            tint = tint_for_offset.get(hour)
            fill = (self._color(f"MAP_BAND_{tint}") if tint is not None
                    else self._color("MAP_OCEAN"))
            self.canvas.create_rectangle(x0, 0, x1, self._ruler_height,
                                         fill=fill, outline="", tags="ruler")
            if x1 - x0 >= self._px(14):
                label = "UTC" if hour == 0 else f"{hour:+d}"
                self.canvas.create_text(
                    (x0 + x1) / 2.0, self._ruler_height / 2.0, text=label,
                    fill=self._color("MAP_RULER_FG"), font=font, tags="ruler")

    def _draw_selection(self) -> None:
        if self._base_map is None or self._board is not None:
            return  # the board is showing; end_tzd composes the selection
        self._map_photo = self._compose_map()
        self.canvas.itemconfigure("map", image=self._map_photo)

    # -- Timezone Dominator easter egg ---------------------------------------------------------
    # Three quick clicks on Afghanistan turn this map into the Timezone Dominator board
    # (see _egg_click). The window then stays open behind the lobby, which
    # is kept above it like a sheet: closing the lobby turns the board back
    # into the map, starting a game closes this window (the game window has
    # its own board).

    #: Crossfade length: 12 frames of 50 ms, then a beat to take in the board.
    MORPH_FRAMES = 12
    MORPH_FRAME_MS = 50
    MORPH_HOLD_MS = 450

    def _play_tzd(self) -> None:
        """Turn this map into the Timezone Dominator board, then open the lobby over it.

        The game modules are imported here, not at the top: the picker must
        not depend on them, and most people never find the way in."""
        if self._on_play is None or self._transforming:
            return
        if self._board is not None:  # already the board: bring Timezone Dominator back up
            self._on_play(self)
            return
        import weeknumber_tzd_board as tzd_board
        import weeknumber_tzd_rules as tzd_rules
        board = tzd_rules.load_board()
        if board is None or self._map_image is None:
            self.status.configure(text="Timezone Dominator is unavailable on this system.")
            return
        self._board = board
        self._set_controls_enabled(False)
        self.status.configure(text="Conquer the world\u2026")
        self._board_image = tzd_board.render_preview(
            board, self.theme, self._map_width, self._map_height)

        def opened() -> None:
            try:
                self._on_play(self)
            except Exception:
                log.exception("Could not start Timezone Dominator from the time-zone map")
                self.end_tzd()

        def shown() -> None:
            self._base_map = None  # not needed while the board shows; rebuilt after
            self.top.after(self.MORPH_HOLD_MS, opened)

        self._morph(self._map_image, self._board_image, to_board=True, done=shown)

    def keep_above(self, window: tk.Toplevel) -> None:
        """Keep `window` (the lobby) above this one: clicking the board
        brings it back to the front."""
        self._sheet = window

    def end_tzd(self, game_started: bool = False) -> None:
        """The lobby closed: a started game takes over from this window,
        otherwise the board turns back into the time-zone map."""
        self._sheet = None
        try:
            if not self.top.winfo_exists():
                return
        except tk.TclError:
            return
        if game_started:
            self.close()
            return
        if self._board is None or self._transforming:
            return
        board_image = self._board_image
        if self._base_map is None:
            self._base_map = self._render_base()
        self._map_photo = self._compose_map()  # the map, current selection

        def done() -> None:
            self._board = None
            self._board_image = None
            self._set_controls_enabled(True)
            self._render_status()

        self._morph(board_image, self._map_image, to_board=False, done=done)

    def _morph(self, start, end, to_board: bool, done) -> None:
        """Crossfade the map between two display-size images; halfway, the
        strip above it swaps between the offset ruler and the continent
        legend."""
        self._transforming = True
        start, end = start.convert("RGB"), end.convert("RGB")

        def step(frame: int) -> None:
            try:
                if not self.top.winfo_exists():
                    return
            except tk.TclError:
                return
            t = frame / self.MORPH_FRAMES
            eased = t * t * (3 - 2 * t)  # smoothstep: ease in and out
            self._map_photo = ImageTk.PhotoImage(Image.blend(start, end, eased))
            self.canvas.itemconfigure("map", image=self._map_photo)
            if frame == self.MORPH_FRAMES // 2:
                self._draw_strip(legend=to_board)
            if frame < self.MORPH_FRAMES:
                self.top.after(self.MORPH_FRAME_MS, step, frame + 1)
            else:
                self._transforming = False
                done()

        step(1)

    def _draw_strip(self, legend: bool) -> None:
        """The strip above the map: the offset ruler, or the board's
        continent legend in the same cells."""
        self.canvas.delete("ruler")
        if not legend:
            self._draw_ruler()
            return
        import weeknumber_tzd_board as tzd_board
        tzd_board.draw_continent_legend(
            self.canvas, self._board, self.theme, self._map_width,
            self._ruler_height, (self._family, -max(7, self._px(9))), tags="ruler")

    def _show_board(self) -> None:
        """Repaint the board after a theme change while it is showing."""
        import weeknumber_tzd_board as tzd_board
        self._board_image = tzd_board.render_preview(
            self._board, self.theme, self._map_width, self._map_height)
        self._map_photo = ImageTk.PhotoImage(self._board_image)
        self.canvas.itemconfigure("map", image=self._map_photo)
        self._draw_strip(legend=True)
        self._base_map = None  # _draw_map rebuilt it for the new theme

    def _set_controls_enabled(self, enabled: bool) -> None:
        """The time-zone controls rest while the map is the board."""
        self.combo.configure(state="readonly" if enabled and self._zones else "disabled")
        self.switch.set_enabled(enabled)
        self.switch_label.configure(cursor="hand2" if enabled else "")
        self.canvas.configure(cursor="hand2" if enabled and self._zones else "")

    def _on_focus_in(self, event) -> None:
        sheet = self._sheet
        if sheet is None:
            return
        try:
            if sheet.winfo_exists():
                sheet.lift()
                sheet.focus_force()
                return
        except tk.TclError:
            pass
        self._sheet = None

    def _on_map_click(self, event) -> None:
        if not self._zones or self._board is not None:
            return
        if event.y < self._ruler_height:
            return  # the ruler is a legend, not a target
        point = self._renderer.from_px(event.x, event.y - self._ruler_height)
        if point is None:  # outside the map body
            return
        if self._egg_click(event.time, point):
            return
        zone = nearest_zone(point[0], point[1])
        if zone is not None:
            self._select(zone.key)

    def _egg_click(self, time_ms: int, point: "tuple[float, float]") -> bool:
        """Count a click towards the easter egg; True once it fires. The
        first clicks select Afghanistan's zone as usual, so the zone chosen
        before them is put back when the third one starts the game."""
        if self._on_play is None or not is_egg_point(*point):
            self._egg.reset()
            return False
        if not self._egg.started:
            self._egg_before = self._selected_key
        if not self._egg.hit(time_ms):
            return False
        if self._egg_before != self._selected_key:
            self._restore_selection(self._egg_before)
        self._play_tzd()
        return True

    def _restore_selection(self, key: "str | None") -> None:
        self._selected_key = key
        self.settings.timezone_key = key
        self.settings.save()
        self._sync_combo_selection()
        self._draw_selection()
        self._render_status()
        self._notify()

    # -- selection ----------------------------------------------------------
    def _select(self, key: str) -> None:
        if key == self._selected_key:
            return
        self._selected_key = key
        self.settings.timezone_key = key
        self.settings.save()
        self._sync_combo_selection()
        self._draw_selection()
        self._render_status()
        self._notify()

    def _sync_combo_selection(self) -> None:
        index = next((i for i, z in enumerate(self._zones)
                      if z.key == self._selected_key), None)
        if index is None:
            self.combo_var.set("")
            return
        self.combo.current(index)
        self.combo.selection_clear()

    def _on_combo_selected(self, event=None) -> None:
        index = self.combo.current()
        if 0 <= index < len(self._zones):
            self._select(self._zones[index].key)
        self.combo.selection_clear()
        self.top.focus_set()

    def _on_toggle_enabled(self, value: bool) -> None:
        self.settings.timezone_enabled = value
        if value and self.settings.timezone_key is None:
            # Turning the clock on with nothing chosen adopts the machine's own
            # zone, so the calendar never shows an empty slot.
            fallback = self._selected_key or system_zone_key()
            if fallback is not None:
                self.settings.timezone_key = fallback
                self._selected_key = fallback
                self._sync_combo_selection()
                self._draw_selection()
        self.settings.save()
        self._render_status()
        self._notify()

    def _notify(self) -> None:
        if self._on_change is None:
            return
        try:
            self._on_change()
        except Exception:
            log.exception("Time-zone change notification failed")

    # -- clock --------------------------------------------------------------
    def _render_status(self) -> None:
        if self._board is not None:  # the easter egg owns the status line
            return
        if not self._zones:
            self.status.configure(text="No time zones available on this system.")
            return
        key = self._selected_key
        if key is None:
            self.status.configure(text="Select a time zone.")
            return
        reading = zone_now_and_offset(key)
        if reading is None:
            self.status.configure(text="Time unavailable for this zone.")
            return
        zone = find_zone(key)
        name = zone.city if zone is not None else "System time zone"
        now, offset = reading
        self.status.configure(
            text=f"{name}  \u00b7  {format_offset(offset)}  \u00b7  {now:%H:%M}")

    def _tick_clock(self) -> None:
        self._clock_after = None
        try:
            if not self.top.winfo_exists():
                return
            self._render_status()
            self._clock_after = self.top.after(CLOCK_TICK_MS, self._tick_clock)
        except tk.TclError:
            self._clock_after = None

    # -- teardown -------------------------------------------------------------
    def _on_close(self) -> None:
        if self._clock_after is not None:
            try:
                self.top.after_cancel(self._clock_after)
            except Exception:
                pass
            self._clock_after = None
        drop_map(self)
        # A few MB of bitmaps: gone now, not whenever the collector runs.
        self._base_map = self._map_image = self._board_image = None
