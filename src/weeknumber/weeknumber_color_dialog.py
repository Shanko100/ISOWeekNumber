"""Icon colour picker for WeekNumber, in the app's own style.

It replaces the system colour dialog, which looked like no other WeekNumber
window and, being modal to the hidden root window, did not come up while
another WeekNumber window was open. This is an ordinary single-instance
titled window like the others: it opens (or comes to the front) whatever else
is open, follows the light and dark palette, and only changes the icon when
Apply is pressed -- the same contract as the system dialog's OK.

Any colour can be chosen: a saturation/brightness field with a hue slider,
36 standard colours
(three rows: the app's hues, light tints and deep shades), or a #RRGGBB
value typed in. Two previews show the week number
on a dark and a light taskbar (menu bar on macOS) as it will look.
"""

from __future__ import annotations

import colorsys
import datetime as dt
import tkinter as tk
from typing import TYPE_CHECKING

from PIL import Image, ImageDraw, ImageTk

from weeknumber_core import IS_MACOS, SettingsStore, log, normalize_hex
from weeknumber_dialog import TitledDialog
from weeknumber_widgets import PillButton, rounded

if TYPE_CHECKING:
    from collections.abc import Callable

#: (colour, name) presets, PRESET_COLUMNS to a row: dark shades on top, the
#: plain colours in the middle and light tints at the bottom, one colour
#: family per column -- black/silver/white, slate, sky, blue, violet, pink,
#: red, orange, yellow, green, teal and brown.
PRESETS = (
    # dark
    ("#000000", "Black"), ("#334155", "Ink"), ("#0369A1", "Ocean"),
    ("#1E3A8A", "Navy"), ("#5B21B6", "Purple"), ("#BE185D", "Magenta"),
    ("#B91C1C", "Crimson"), ("#C2410C", "Rust"), ("#A16207", "Gold"),
    ("#15803D", "Forest"), ("#0F766E", "Pine"), ("#5C3317", "Chocolate"),
    # normal
    ("#9CA3AF", "Silver"), ("#64748B", "Slate"), ("#4CC2FF", "Sky"),
    ("#2F6FED", "Blue"), ("#8B5CF6", "Violet"), ("#EC4899", "Pink"),
    ("#EF4444", "Red"), ("#F59E0B", "Amber"), ("#FACC15", "Yellow"),
    ("#22C55E", "Green"), ("#14B8A6", "Teal"), ("#8B5A2B", "Brown"),
    # light
    ("#FFFFFF", "White"), ("#CBD5E1", "Mist"), ("#BAE6FD", "Ice"),
    ("#93C5FD", "Light blue"), ("#C4B5FD", "Lavender"), ("#F9A8D4", "Rose"),
    ("#FCA5A5", "Salmon"), ("#FDBA74", "Peach"), ("#FEF08A", "Cream"),
    ("#86EFAC", "Mint"), ("#99F6E4", "Aqua"), ("#D2B48C", "Tan"),
)
PRESET_COLUMNS = 12
PRESET_ROWS = -(-len(PRESETS) // PRESET_COLUMNS)
#: What the previews sit on: a dark and a light taskbar / menu bar.
PREVIEW_DARK = "#202020"
PREVIEW_LIGHT = "#F3F3F3"


def hex_to_rgb(value: str) -> "tuple[int, int, int]":
    value = normalize_hex(value).lstrip("#")
    return int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16)


def rgb_to_hex(rgb: "tuple[float, float, float]") -> str:
    return "#" + "".join(f"{max(0, min(255, round(c))):02X}" for c in rgb)


def hex_to_hsv(value: str) -> "tuple[float, float, float]":
    r, g, b = hex_to_rgb(value)
    return colorsys.rgb_to_hsv(r / 255.0, g / 255.0, b / 255.0)


def hsv_to_hex(h: float, s: float, v: float) -> str:
    r, g, b = colorsys.hsv_to_rgb(h % 1.0, max(0.0, min(1.0, s)), max(0.0, min(1.0, v)))
    return rgb_to_hex((r * 255, g * 255, b * 255))


def parse_hex(text: str) -> "str | None":
    """A typed colour as #RRGGBB, or None. normalize_hex turns anything
    invalid into white, which is right for settings but not for typing."""
    value = text.strip()
    if not value.startswith("#"):
        value = "#" + value
    if len(value) != 7 or any(c not in "0123456789abcdefABCDEF" for c in value[1:]):
        return None
    return value.upper()


class ColorPickerDialog(TitledDialog):
    """Single-instance colour picker; `on_choose(color)` runs on Apply."""

    TITLE = "Icon Color"
    MARGIN_PX = 24
    FIELD_W_PX = 236
    FIELD_H_PX = 164
    HUE_H_PX = 14
    GAP_PX = 14
    KNOB_PX = 18
    PREVIEW_W_PX = 112
    PREVIEW_H_PX = 62
    SWATCH_PX = 22
    SWATCH_GAP_PX = 6

    @classmethod
    def show(cls, master: tk.Misc, settings: SettingsStore, color: str,
             on_choose: "Callable[[str], None]") -> None:
        """Open the picker, or bring the open one forward."""
        super().show(master, settings, color, on_choose)

    def __init__(self, master: tk.Misc, settings: SettingsStore, color: str,
                 on_choose: "Callable[[str], None]") -> None:
        work = self._init_window(master, settings)
        self._on_choose = on_choose
        self._color = normalize_hex(color)
        self._h, self._s, self._v = hex_to_hsv(self._color)
        self._drag: "str | None" = None       # "field" | "hue" while dragging
        self._images: dict[str, ImageTk.PhotoImage] = {}
        self._field_hue: "float | None" = None
        self._week = dt.date.today().isocalendar()[1]
        self.hex_var = tk.StringVar(self.top, value=self._color)
        self._build()
        self._apply_theme()
        self._center(work, vertical=0.4)
        self.top.deiconify()
        self._sync_titlebar()
        self._raise()

    # -- construction ---------------------------------------------------------
    def _build(self) -> None:
        margin = self._px(self.MARGIN_PX)
        self.card = tk.Frame(self.top, padx=margin, pady=self._px(20))
        self.card.pack(fill="both", expand=True)
        self.title = tk.Label(self.card, text="Icon color", font=self._font(19, bold=True),
                              anchor="w")
        self.title.pack(fill="x")
        where = "menu bar" if IS_MACOS else "taskbar"
        self.subtitle = tk.Label(self.card, anchor="w", font=self._font(11),
                                 text=f"The colour of the week number in the {where}.")
        self.subtitle.pack(fill="x", pady=(self._px(4), self._px(16)))

        self.body = tk.Frame(self.card)
        self.body.pack(fill="x")
        field_w, field_h = self._px(self.FIELD_W_PX), self._px(self.FIELD_H_PX)
        knob = self._px(self.KNOB_PX)
        # The knobs may overhang the field and the slider by half their size.
        self._pad = knob // 2 + 1
        self._hue_y = self._pad + field_h + self._px(self.GAP_PX)
        height = self._hue_y + self._px(self.HUE_H_PX) + self._pad
        self.canvas = tk.Canvas(self.body, width=field_w + 2 * self._pad, height=height,
                                bd=0, highlightthickness=0, cursor="crosshair")
        self.canvas.pack(side="left")
        self.canvas.bind("<ButtonPress-1>", self._press)
        self.canvas.bind("<B1-Motion>", self._motion)
        self.canvas.bind("<ButtonRelease-1>", lambda e: setattr(self, "_drag", None))

        self.previews = tk.Frame(self.body)
        self.previews.pack(side="left", fill="y", padx=(self._px(16), 0))
        self.preview_dark = tk.Label(self.previews, bd=0, highlightthickness=0)
        self.preview_dark.pack(anchor="n")
        self.preview_dark_label = tk.Label(self.previews, text="Dark", font=self._font(10))
        self.preview_dark_label.pack(anchor="n", pady=(self._px(3), self._px(10)))
        self.preview_light = tk.Label(self.previews, bd=0, highlightthickness=0)
        self.preview_light.pack(anchor="n")
        self.preview_light_label = tk.Label(self.previews, text="Light", font=self._font(10))
        self.preview_light_label.pack(anchor="n", pady=(self._px(3), 0))

        size, gap = self._px(self.SWATCH_PX), self._px(self.SWATCH_GAP_PX)
        self.swatches = tk.Canvas(self.card, height=PRESET_ROWS * (size + gap) - gap + 4,
                                  bd=0, highlightthickness=0,
                                  width=PRESET_COLUMNS * (size + gap) - gap + 4)
        self.swatches.pack(anchor="w", pady=(self._px(16), 0))
        self.swatches.bind("<Button-1>", self._swatch_click)
        self.swatches.configure(cursor="hand2")

        self.hex_row = tk.Frame(self.card)
        self.hex_row.pack(fill="x", pady=(self._px(14), 0))
        self.hex_label = tk.Label(self.hex_row, text="Hex", font=self._font(11, bold=True))
        self.hex_label.pack(side="left", padx=(0, self._px(10)))
        self.hex_entry = tk.Entry(self.hex_row, textvariable=self.hex_var, width=9,
                                  font=self._font(11), relief="flat", bd=self._px(4),
                                  highlightthickness=1)
        self.hex_entry.pack(side="left")
        self.hex_entry.bind("<Return>", self._hex_commit)
        self.hex_entry.bind("<FocusOut>", self._hex_commit)
        self.hex_entry.bind("<KeyRelease>", self._hex_typed)
        self.reset_button = PillButton(self.hex_row, "Default", self._font(10, bold=True),
                                       self.scale, lambda: self._set_color("#FFFFFF"),
                                       variant="chip")
        self.reset_button.pack(side="left", padx=(self._px(10), 0))

        self.footer = tk.Frame(self.card)
        self.footer.pack(fill="x", pady=(self._px(20), 0))
        font = self._font(11, bold=True)
        self.apply_button = PillButton(self.footer, "Apply", font, self.scale, self._apply)
        self.apply_button.pack(side="right")
        self.cancel_button = PillButton(self.footer, "Cancel", font, self.scale, self.close,
                                        primary=False)
        self.cancel_button.pack(side="right", padx=(0, self._px(8)))
        # Enter applies, except where it already means something: the hex
        # field takes the typed value, a focused button presses itself.
        self.top.bind("<Return>", lambda e: None if isinstance(e.widget, (tk.Entry, PillButton))
                      else self._apply())

    # -- artwork ----------------------------------------------------------------
    def _field_image(self) -> Image.Image:
        """Saturation left to right, brightness top to bottom, for the hue."""
        w, h = self._px(self.FIELD_W_PX), self._px(self.FIELD_H_PX)
        hue = tuple(round(c * 255) for c in colorsys.hsv_to_rgb(self._h, 1.0, 1.0))
        across = Image.linear_gradient("L").rotate(90).resize((w, h))  # 0 left -> 255 right
        down = Image.linear_gradient("L").resize((w, h))      # 0 top -> 255 bottom
        tinted = Image.composite(Image.new("RGB", (w, h), hue),
                                 Image.new("RGB", (w, h), "#FFFFFF"), across)
        shaded = Image.composite(Image.new("RGB", (w, h), "#000000"), tinted, down)
        return self._clip(shaded, self._px(8))

    def _hue_image(self) -> Image.Image:
        w, h = self._px(self.FIELD_W_PX), self._px(self.HUE_H_PX)
        strip = Image.new("RGB", (w, 1))
        strip.putdata([tuple(round(c * 255) for c in colorsys.hsv_to_rgb(x / max(1, w - 1), 1, 1))
                       for x in range(w)])
        return self._clip(strip.resize((w, h)), h / 2.0)

    def _clip(self, image: Image.Image, radius: float) -> Image.Image:
        """`image` with antialiased rounded corners on the card colour."""
        w, h = image.size
        ss = 4
        mask = Image.new("L", (w * ss, h * ss), 0)
        ImageDraw.Draw(mask).rounded_rectangle((0, 0, w * ss - 1, h * ss - 1),
                                               radius=radius * ss, fill=255)
        out = Image.new("RGB", (w, h), self.theme["CARD_BG"])
        out.paste(image, (0, 0), mask.resize((w, h), Image.LANCZOS))
        return out

    def _knob(self, fill: str) -> Image.Image:
        size, ss = self._px(self.KNOB_PX), 4
        big = Image.new("RGBA", (size * ss, size * ss), (0, 0, 0, 0))
        draw = ImageDraw.Draw(big)
        draw.ellipse((0, 0, size * ss - 1, size * ss - 1), fill=(0, 0, 0, 90))
        draw.ellipse((ss, ss, size * ss - 1 - ss, size * ss - 1 - ss), fill="#FFFFFF")
        inset = round(size * ss * 0.24)
        draw.ellipse((inset, inset, size * ss - 1 - inset, size * ss - 1 - inset), fill=fill)
        return big.resize((size, size), Image.LANCZOS)

    def _preview(self, bg: str) -> Image.Image:
        """The week number in the chosen colour on a taskbar-coloured chip."""
        w, h = self._px(self.PREVIEW_W_PX), self._px(self.PREVIEW_H_PX)
        return rounded(w, h, self._px(10), bg, self.theme["CARD_BG"],
                       outline=self.theme["CARD_BORDER"], outline_width=self._px(1))

    # -- drawing ----------------------------------------------------------------
    def _photo(self, key: str, image: Image.Image) -> ImageTk.PhotoImage:
        photo = ImageTk.PhotoImage(image, master=self.top)
        self._images[key] = photo  # Tk only holds a weak reference
        return photo

    def _draw_canvas(self) -> None:
        canvas = self.canvas
        pad, hue_y = self._pad, self._hue_y
        field_w, field_h = self._px(self.FIELD_W_PX), self._px(self.FIELD_H_PX)
        if self._field_hue != self._h or "field" not in self._images:
            self._field_hue = self._h
            canvas.delete("field")
            canvas.create_image(pad, pad, anchor="nw", tags="field",
                                image=self._photo("field", self._field_image()))
        if "hue" not in self._images:
            canvas.create_image(pad, hue_y, anchor="nw", tags="hue",
                                image=self._photo("hue", self._hue_image()))
        canvas.delete("knob")
        canvas.create_image(pad + self._s * (field_w - 1), pad + (1 - self._v) * (field_h - 1),
                            tags="knob", image=self._photo("knob_field", self._knob(self._color)))
        hue_color = hsv_to_hex(self._h, 1.0, 1.0)
        canvas.create_image(pad + self._h * (field_w - 1), hue_y + self._px(self.HUE_H_PX) / 2,
                            tags="knob", image=self._photo("knob_hue", self._knob(hue_color)))

    def _draw_previews(self) -> None:
        font = self._font(26, bold=self.settings.is_bold)
        for label, bg, key in ((self.preview_dark, PREVIEW_DARK, "dark"),
                               (self.preview_light, PREVIEW_LIGHT, "light")):
            photo = self._images.get(f"preview_{key}")
            if photo is None:
                photo = self._photo(f"preview_{key}", self._preview(bg))
            label.configure(image=photo, text=str(self._week), compound="center",
                            font=font, fg=self._color, bg=self.theme["CARD_BG"],
                            activebackground=self.theme["CARD_BG"])

    def _draw_swatches(self) -> None:
        canvas = self.swatches
        canvas.delete("all")
        size, gap = self._px(self.SWATCH_PX), self._px(self.SWATCH_GAP_PX)
        theme = self.theme
        for index, (color, _name) in enumerate(PRESETS):
            row, column = divmod(index, PRESET_COLUMNS)
            x, y = 2 + column * (size + gap), 2 + row * (size + gap)
            selected = color == self._color
            key = f"swatch{index}{'s' if selected else ''}"
            photo = self._images.get(key)
            if photo is None:
                ss = 4
                big = Image.new("RGB", (size * ss, size * ss), theme["CARD_BG"])
                draw = ImageDraw.Draw(big)
                if selected:
                    draw.ellipse((0, 0, size * ss - 1, size * ss - 1), fill=theme["ACCENT"])
                    inner = round(size * ss * 0.12)
                    draw.ellipse((inner, inner, size * ss - 1 - inner, size * ss - 1 - inner),
                                 fill=theme["CARD_BG"])
                    inset = round(size * ss * 0.2)
                else:
                    inset = round(size * ss * 0.08)
                draw.ellipse((inset, inset, size * ss - 1 - inset, size * ss - 1 - inset),
                             fill=color, outline=theme["INPUT_BORDER"], width=ss)
                photo = self._photo(key, big.resize((size, size), Image.LANCZOS))
            canvas.create_image(x, y, anchor="nw", image=photo, tags=(f"swatch{index}",))

    def _redraw(self) -> None:
        try:
            self._draw_canvas()
            self._draw_previews()
            self._draw_swatches()
        except tk.TclError:
            log.debug("Colour picker redraw failed", exc_info=True)

    # -- state --------------------------------------------------------------------
    def _set_hsv(self, h: float, s: float, v: float) -> None:
        self._h, self._s, self._v = h % 1.0, max(0.0, min(1.0, s)), max(0.0, min(1.0, v))
        self._color = hsv_to_hex(self._h, self._s, self._v)
        if self.hex_var.get().upper() != self._color:
            self.hex_var.set(self._color)
        self._redraw()

    def _set_color(self, color: str, from_entry: bool = False) -> None:
        color = normalize_hex(color)
        h, s, v = hex_to_hsv(color)
        if s == 0.0 or v == 0.0:
            h = self._h  # grey, white, black: keep the hue the slider shows
        self._h, self._s, self._v = h, s, v
        self._color = color
        if not from_entry:
            self.hex_var.set(color)
        self._redraw()

    # -- interaction ----------------------------------------------------------------
    def _region(self, x: float, y: float) -> "str | None":
        pad = self._pad
        field_w, field_h = self._px(self.FIELD_W_PX), self._px(self.FIELD_H_PX)
        if pad - 4 <= x <= pad + field_w + 4 and pad - 4 <= y <= pad + field_h + 4:
            return "field"
        hue_h = self._px(self.HUE_H_PX)
        if self._hue_y - self._pad <= y <= self._hue_y + hue_h + self._pad:
            return "hue"
        return None

    def _pick(self, x: float, y: float) -> None:
        pad = self._pad
        field_w, field_h = self._px(self.FIELD_W_PX), self._px(self.FIELD_H_PX)
        fx = max(0.0, min(1.0, (x - pad) / max(1, field_w - 1)))
        if self._drag == "field":
            fy = max(0.0, min(1.0, (y - pad) / max(1, field_h - 1)))
            self._set_hsv(self._h, fx, 1.0 - fy)
        elif self._drag == "hue":
            self._set_hsv(min(fx, 0.9999), self._s, self._v)

    def _press(self, event) -> None:
        self._drag = self._region(event.x, event.y)
        self._pick(event.x, event.y)

    def _motion(self, event) -> None:
        self._pick(event.x, event.y)

    def _swatch_click(self, event) -> None:
        index = self.swatch_at(event.x, event.y)
        if index is not None:
            self._set_color(PRESETS[index][0])

    def swatch_at(self, x: int, y: int) -> "int | None":
        """The preset under canvas point (x, y); None in the gaps."""
        size, gap = self._px(self.SWATCH_PX), self._px(self.SWATCH_GAP_PX)
        if x < 2 or y < 2:
            return None
        column, cx = divmod(x - 2, size + gap)
        row, cy = divmod(y - 2, size + gap)
        index = int(row) * PRESET_COLUMNS + int(column)
        if column < PRESET_COLUMNS and cx <= size and cy <= size and 0 <= index < len(PRESETS):
            return index
        return None

    def _hex_typed(self, event=None) -> None:
        color = parse_hex(self.hex_var.get())
        if color is not None and color != self._color:
            self._set_color(color, from_entry=True)

    def _hex_commit(self, event=None) -> "str | None":
        color = parse_hex(self.hex_var.get())
        if color is None:
            self.hex_var.set(self._color)
            if event is not None and event.type == tk.EventType.KeyPress:
                self.top.bell()
            return "break"
        self._set_color(color)
        return None

    def _apply(self) -> None:
        color = parse_hex(self.hex_var.get()) or self._color
        try:
            self._on_choose(normalize_hex(color))
        except Exception:
            log.exception("Could not apply the icon colour")
        self.close()

    # -- theme ----------------------------------------------------------------------
    def _apply_theme(self) -> None:
        super()._apply_theme()
        theme = self.theme
        bg, fg, muted = theme["CARD_BG"], theme["TITLE_FG"], theme["YEAR_FG"]
        self.top.configure(bg=bg)
        for widget in (self.card, self.body, self.previews, self.hex_row, self.footer):
            widget.configure(bg=bg)
        for canvas in (self.canvas, self.swatches):
            canvas.configure(bg=bg)
        self.title.configure(bg=bg, fg=fg)
        self.hex_label.configure(bg=bg, fg=fg)
        for label in (self.subtitle, self.preview_dark_label, self.preview_light_label):
            label.configure(bg=bg, fg=muted)
        self.hex_entry.configure(bg=theme["INPUT_BG"], fg=fg, insertbackground=fg,
                                 highlightbackground=theme["INPUT_BORDER"],
                                 highlightcolor=theme["ACCENT"])
        for button in (self.apply_button, self.cancel_button, self.reset_button):
            button.apply_theme(theme)
        # Every image bakes in the card colour: draw them all again.
        self.canvas.delete("all")
        self._images.clear()
        self._field_hue = None
        self._redraw()
