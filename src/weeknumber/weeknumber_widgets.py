"""Hand-drawn controls shared by WeekNumber's dialogs.

Tk's native Checkbutton and Button render as Motif-era controls on Windows
that neither scale cleanly nor take a dark palette, and on macOS the Aqua
button ignores colours altogether. These are plain Labels showing an
antialiased Pillow image, so they look the same on both platforms and at
every DPI.
"""

from __future__ import annotations

import tkinter as tk
import tkinter.font as tkfont
from tkinter import ttk
from typing import TYPE_CHECKING

from PIL import Image, ImageDraw, ImageTk

if TYPE_CHECKING:
    from collections.abc import Callable


def shade(color: str, factor: float) -> str:
    """Lighten (factor > 1) or darken (factor < 1) a #rrggbb colour."""
    value = color.lstrip("#")
    channels = (int(value[i:i + 2], 16) for i in (0, 2, 4))
    return "#" + "".join(f"{max(0, min(255, round(c * factor))):02x}"
                         for c in channels)


def blend(color: str, towards: str, amount: float) -> str:
    """Mix two #rrggbb colours. `amount` is how far to move toward `towards`."""
    a = color.lstrip("#")
    b = towards.lstrip("#")
    parts = []
    for i in (0, 2, 4):
        first, second = int(a[i:i + 2], 16), int(b[i:i + 2], 16)
        parts.append(f"{round(first + (second - first) * amount):02x}")
    return "#" + "".join(parts)


def rounded(width: int, height: int, radius: float, fill: str, bg: "str | None",
            supersample: int = 4, outline: "str | None" = None,
            outline_width: int = 1) -> "Image.Image":
    """A rounded rectangle rendered smooth.

    Tk's canvas has no antialiasing, so capsules drawn from arcs come out with
    visibly stepped edges. Pillow is already a dependency for the tray icon, so
    the control is drawn oversized and resampled down instead -- the same
    technique IconRenderer uses. With a `bg` colour the result is composited
    onto that known background, which is exact on every Tk build; with
    ``bg=None`` it keeps transparent corners for surfaces Tk can't paint, such
    as the macOS glass flyout. `outline_width` is in display pixels.
    """
    scale = max(1, supersample)
    mode, background = ("RGB", bg) if bg is not None else ("RGBA", (0, 0, 0, 0))
    image = Image.new(mode, (width * scale, height * scale), background)
    ImageDraw.Draw(image).rounded_rectangle(
        (0, 0, width * scale - 1, height * scale - 1),
        radius=radius * scale, fill=fill, outline=outline,
        width=scale * max(1, outline_width) if outline else 0)
    return image.resize((width, height), Image.LANCZOS)


def style_combobox(widget: tk.Misc, style_name: str, theme: dict[str, str],
                   scale: float) -> None:
    """Theme a readonly ttk.Combobox style for the active palette.

    The native themes ignore background colours on a readonly combobox, so
    dark mode needs 'clam'. Switching the theme is interpreter-wide, which is
    fine because every ttk widget in the app is a combobox styled here.
    """
    def px(value: float) -> int:
        return max(1, round(value * scale))

    style = ttk.Style(widget)
    if style.theme_use() != "clam":
        style.theme_use("clam")
    field, fg, border = theme["INPUT_BG"], theme["TITLE_FG"], theme["INPUT_BORDER"]
    style.configure(
        style_name, fieldbackground=field, background=field, foreground=fg,
        arrowcolor=theme["YEAR_FG"], bordercolor=border, lightcolor=field,
        darkcolor=field, selectbackground=field, selectforeground=fg,
        arrowsize=px(12), padding=px(4))
    style.map(
        style_name,
        fieldbackground=[("readonly", field), ("disabled", field)],
        foreground=[("readonly", fg), ("disabled", theme["WEEK_NUM_FG"])],
        background=[("readonly", field), ("active", field)],
        bordercolor=[("focus", theme["ACCENT"])],
        selectbackground=[("readonly", field)],
        selectforeground=[("readonly", fg)])
    # The popdown list is a plain Tk listbox created by Tcl on first open, so
    # it can only be coloured through the option database.
    for option, key in (("background", "MENU_BG"), ("foreground", "MENU_FG"),
                        ("selectBackground", "MENU_ACTIVE_BG"),
                        ("selectForeground", "MENU_ACTIVE_FG")):
        widget.option_add(f"*TCombobox*Listbox.{option}", theme[key])


class Switch(tk.Label):
    """An on/off switch: capsule track, circular knob, both antialiased."""

    def __init__(self, master: tk.Misc, scale: float, value: bool,
                 command: "Callable[[bool], None]") -> None:
        self._scale = scale
        self._width = self._px(40)
        self._height = self._px(22)
        super().__init__(master, bd=0, highlightthickness=0, cursor="hand2",
                         takefocus=True)
        self._value = bool(value)
        self._command = command
        self._enabled = True
        self._theme: dict[str, str] = {}
        self._images: dict[bool, "ImageTk.PhotoImage"] = {}
        self.bind("<Button-1>", lambda e: self.toggle())
        self.bind("<space>", lambda e: self.toggle())
        self.bind("<Return>", lambda e: self.toggle())

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def toggle(self) -> None:
        if not self._enabled:
            return
        self._value = not self._value
        self.redraw()
        self._command(self._value)

    def set_enabled(self, enabled: bool) -> None:
        self._enabled = bool(enabled)
        self.configure(cursor="hand2" if self._enabled else "")

    def apply_theme(self, theme: dict[str, str]) -> None:
        self._theme = theme
        self.configure(bg=theme["CARD_BG"])
        self._images.clear()
        self.redraw()

    def _image(self, value: bool) -> "ImageTk.PhotoImage":
        cached = self._images.get(value)
        if cached is not None:
            return cached
        bg = self._theme["CARD_BG"]
        track = self._theme["ACCENT"] if value else self._theme["TOGGLE_TRACK_OFF"]
        supersample = 4
        # Track and knob drawn at the same oversample, then resampled once.
        big = Image.new("RGB", (self._width * supersample,
                                self._height * supersample), bg)
        ImageDraw.Draw(big).rounded_rectangle(
            (0, 0, self._width * supersample - 1, self._height * supersample - 1),
            radius=self._height * supersample / 2.0, fill=track)
        inset = self._height * supersample * 0.16
        diameter = self._height * supersample - 2 * inset
        left = (self._width * supersample - inset - diameter) if value else inset
        ImageDraw.Draw(big).ellipse(
            (left, inset, left + diameter, inset + diameter),
            fill=self._theme["TOGGLE_KNOB"])
        image = big.resize((self._width, self._height), Image.LANCZOS)
        photo = ImageTk.PhotoImage(image)
        self._images[value] = photo  # the widget only holds a weak reference
        return photo

    def redraw(self) -> None:
        if not self._theme:
            return
        self.configure(image=self._image(self._value))


class PillButton(tk.Label):
    """A capsule button with hover, pressed and disabled states and
    antialiased edges. Every clickable button in WeekNumber is one of these.

    Variants:
      "primary"    solid accent, for the main action of a window
      "secondary"  quiet outlined capsule for the actions beside it
      "chip"       borderless accent text that shows its capsule on hover --
                   the calendar footer's Today and clock buttons
      "icon"       a quiet round button for a single glyph

    Like a native button it fires on release, and only if the pointer is
    still over it, so a press can be cancelled by sliding off. `min_width_px`
    lets a column of buttons share one width. `transparent` keeps the
    capsule's corners clear for surfaces Tk cannot paint (the macOS glass
    flyout) instead of compositing onto CARD_BG.
    """

    _PAD = {"primary": (18, 7), "secondary": (18, 7), "chip": (10, 3), "icon": (6, 4)}

    def __init__(self, master: tk.Misc, text: str, font, scale: float,
                 command: "Callable[[], None]", primary: bool = True,
                 min_width_px: int = 0, variant: "str | None" = None,
                 transparent: bool = False,
                 on_state: "Callable[[str], None] | None" = None) -> None:
        self._scale = scale
        self._font = font
        self._variant = variant or ("primary" if primary else "secondary")
        self._transparent = transparent
        self._min_width = self._px(min_width_px)
        self._enabled = True
        self._theme: dict[str, str] = {}
        self._hover = False
        self._pressed = False
        self._flash = False
        self._pulse_after: "str | None" = None
        self._images: dict[tuple[str, int, int], "ImageTk.PhotoImage"] = {}
        self._command = command
        #: Told the visual state ("rest", "hover", "pressed", "disabled") on
        #: every change; on a transparent surface the owner draws the capsule.
        self._on_state = on_state
        self.visual_state = "rest"
        super().__init__(master, text=text, font=font, compound="center",
                         bd=0, highlightthickness=0, padx=0, pady=0,
                         cursor="hand2", takefocus=True)
        self._measure(text)
        self.bind("<ButtonPress-1>", self._on_press)
        self.bind("<ButtonRelease-1>", self._on_release)
        self.bind("<Return>", lambda e: self._invoke())
        self.bind("<space>", lambda e: self._invoke())
        self.bind("<Enter>", lambda e: self._set_hover(True))
        self.bind("<Leave>", lambda e: self._set_hover(False))

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def _measure(self, text: str) -> None:
        font = tkfont.Font(root=self, font=self._font)
        pad_x, pad_y = self._PAD[self._variant]
        self._width = max(font.measure(text) + 2 * self._px(pad_x), self._min_width)
        self._height = font.metrics("linespace") + 2 * self._px(pad_y)
        if self._variant == "icon":  # a capsule as wide as it is tall: a circle
            self._width = self._height = max(self._width, self._height)

    def _invoke(self) -> None:
        if self._enabled:
            self._command()

    def _on_press(self, event) -> None:
        if self._enabled:
            self._pressed = True
            self.redraw()

    def _on_release(self, event) -> None:
        was_pressed, self._pressed = self._pressed, False
        inside = 0 <= event.x < self.winfo_width() and 0 <= event.y < self.winfo_height()
        self.redraw()
        if was_pressed and inside:
            self._invoke()

    def _set_hover(self, hover: bool) -> None:
        self._hover = hover
        self.redraw()

    def set_enabled(self, enabled: bool) -> None:
        enabled = bool(enabled)
        if enabled == self._enabled:
            return
        self._enabled = enabled
        self._pressed = False
        self.configure(cursor="hand2" if enabled else "")
        self.redraw()

    def set_variant(self, variant: str) -> None:
        """Switch style, e.g. to "primary" when this becomes the action the
        player has to take next."""
        if variant == self._variant:
            return
        self._variant = variant
        self._measure(self.cget("text"))
        self._images.clear()
        self.redraw()

    def pulse(self, times: int = 3, interval_ms: int = 260) -> None:
        """Draw the eye: show the hover look `times` times."""
        if self._pulse_after is not None:
            self.after_cancel(self._pulse_after)

        def step(n: int) -> None:
            self._pulse_after = None
            try:
                if not self.winfo_exists():
                    return
            except tk.TclError:
                return
            self._flash = n < 2 * times and n % 2 == 0
            self.redraw()
            if n < 2 * times:
                self._pulse_after = self.after(interval_ms, step, n + 1)

        step(0)

    def set_text(self, text: str) -> None:
        if text == self.cget("text"):
            return
        self.configure(text=text)
        old = (self._width, self._height)
        self._measure(text)
        if (self._width, self._height) != old:
            self._images.clear()
        self.redraw()

    def _text_color(self) -> str:
        if not self._enabled:
            return self._theme["WEEK_NUM_FG"]
        if self._variant == "primary":
            return self._theme["TODAY_FG"]
        if self._variant == "icon":
            return self._theme["YEAR_FG"] if self._hover else self._theme["WEEK_NUM_FG"]
        return self._theme["ACCENT"] if self._variant == "chip" else self._theme["TITLE_FG"]

    def apply_theme(self, theme: dict[str, str]) -> None:
        self._theme = theme
        surface = "systemTransparent" if self._transparent else theme["CARD_BG"]
        # Before clearing: Tk looks the shown image up again on every
        # configure, and a freed one fails it ("image ... doesn't exist").
        self.configure(bg=surface, activebackground=surface)
        self._images.clear()
        self.redraw()

    def _state(self) -> str:
        if not self._enabled:
            return "disabled"
        if self._pressed:
            return "pressed"
        return "hover" if self._hover or self._flash else "rest"

    def capsule_fill(self, state: str) -> "str | None":
        """The capsule colour for `state`, or None where the capsule is
        invisible (a chip at rest)."""
        theme = self._theme
        if self._variant == "primary":
            return {"disabled": theme["SEPARATOR_BG"],
                    "pressed": shade(theme["ACCENT"], 0.78),
                    "hover": shade(theme["ACCENT"], 0.88)}.get(state, theme["ACCENT"])
        if self._variant == "secondary":
            return {"pressed": theme["SELECTED_DAY_FILL"],
                    "hover": theme["HOVER_BG"]}.get(state, theme["CARD_BG"])
        return {"pressed": theme["SELECTED_DAY_FILL"], "hover": theme["HOVER_BG"]}.get(state)

    def _image(self, state: str) -> "ImageTk.PhotoImage":
        key = (state, self._width, self._height)
        cached = self._images.get(key)
        if cached is not None:
            return cached
        theme = self._theme
        bg = None if self._transparent else theme["CARD_BG"]
        outline = theme["INPUT_BORDER"] if self._variant == "secondary" else None
        fill = self.capsule_fill(state)
        if fill is None:  # chip and icon: the capsule appears only when used
            fill = (0, 0, 0, 0) if self._transparent else theme["CARD_BG"]
        photo = ImageTk.PhotoImage(
            rounded(self._width, self._height, self._height / 2.0, fill, bg,
                    outline=outline, outline_width=self._px(1)))
        self._images[key] = photo  # the widget only holds a weak reference
        return photo

    def redraw(self) -> None:
        if not self._theme:
            return
        color = self._text_color()
        state = self._state()
        self.configure(image=self._image(state), fg=color, activeforeground=color)
        if state != self.visual_state:
            self.visual_state = state
            if self._on_state is not None:
                self._on_state(state)


class ScrollBar(tk.Canvas):
    """A slim vertical scroll bar in the app's style: a rounded thumb on a
    quiet track, wider and darker while hovered or dragged, like the overlay
    scroll bars of Windows 11 and macOS. Tk's own scroll bar is a native
    control on both systems and ignores the dark palette.

    Use it like a Tk scroll bar: ``ScrollBar(parent, scale, text.yview)``
    and ``text.configure(yscrollcommand=bar.set)``."""

    WIDTH_PX = 12
    REST_PX = 4
    ACTIVE_PX = 8
    MIN_THUMB_PX = 28

    def __init__(self, master: tk.Misc, scale: float,
                 command: "Callable[..., object]") -> None:
        self._scale = scale
        super().__init__(master, width=self._px(self.WIDTH_PX), height=1, bd=0,
                         highlightthickness=0, takefocus=False)
        self._command = command
        self._first, self._last = 0.0, 1.0
        self._theme: dict[str, str] = {}
        self._hover = False
        self._drag: "tuple[float, float] | None" = None   # (pointer y, first)
        self._image: "ImageTk.PhotoImage | None" = None
        self.bind("<Configure>", lambda e: self.redraw())
        self.bind("<Enter>", lambda e: self._set_hover(True))
        self.bind("<Leave>", lambda e: self._set_hover(False))
        self.bind("<ButtonPress-1>", self._press)
        self.bind("<B1-Motion>", self._motion)
        self.bind("<ButtonRelease-1>", self._release)

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def apply_theme(self, theme: dict[str, str]) -> None:
        self._theme = theme
        self.configure(bg=theme["CARD_BG"])
        self.redraw()

    def set(self, first, last) -> None:
        """The view's visible fraction, as Tk's yscrollcommand reports it."""
        self._first, self._last = float(first), float(last)
        self.redraw()

    @property
    def scrollable(self) -> bool:
        return self._last - self._first < 1.0

    def _track(self) -> "tuple[int, int]":
        """(top inset, height) of the track inside the canvas."""
        inset = self._px(2)
        return inset, max(1, self.winfo_height() - 2 * inset)

    def _thumb(self) -> "tuple[float, float]":
        """The thumb's top and bottom inside the track. A thumb held at its
        minimum length moves over the rest of the track, so it still reaches
        both ends."""
        _inset, height = self._track()
        visible = self._last - self._first
        span = max(visible, min(1.0, self._px(self.MIN_THUMB_PX) / height))
        travel = (1.0 - span) / (1.0 - visible) if visible < 1.0 else 0.0
        top = self._first * travel * height
        return top, min(height, top + span * height)

    def redraw(self) -> None:
        """Track and thumb as one antialiased image (canvas ovals are not)."""
        self.delete("all")
        self._image = None
        height = self.winfo_height()
        if not self._theme or not self.scrollable or height < 8:
            return
        width = self.winfo_width() or self._px(self.WIDTH_PX)
        active = self._hover or self._drag is not None
        bar = self._px(self.ACTIVE_PX if active else self.REST_PX)
        inset, track_h = self._track()
        top, bottom = self._thumb()
        top = max(0, round(top))
        bottom = min(track_h, max(top + bar, round(bottom)))
        bg = self._theme["CARD_BG"]
        ss = 4
        big = Image.new("RGB", (bar * ss, track_h * ss), bg)
        draw = ImageDraw.Draw(big)
        if active:
            draw.rounded_rectangle((0, 0, bar * ss - 1, track_h * ss - 1),
                                   radius=bar * ss / 2, fill=self._theme["HOVER_BG"])
        thumb = self._theme["YEAR_FG"] if active else self._theme["TOGGLE_TRACK_OFF"]
        draw.rounded_rectangle((0, top * ss, bar * ss - 1, bottom * ss - 1),
                               radius=bar * ss / 2, fill=thumb)
        self._image = ImageTk.PhotoImage(big.resize((bar, track_h), Image.LANCZOS),
                                         master=self)
        self.create_image(width - inset - bar, inset, anchor="nw", image=self._image)

    def _set_hover(self, hover: bool) -> None:
        self._hover = hover
        self.redraw()

    def _press(self, event) -> None:
        if not self.scrollable:
            return
        inset, _height = self._track()
        top, bottom = self._thumb()
        y = event.y - inset
        if top <= y <= bottom:
            self._drag = (event.y, self._first)
        else:  # a click on the track pages toward it, like a native bar
            self._command("scroll", -1 if y < top else 1, "pages")
        self.redraw()

    def _motion(self, event) -> None:
        if self._drag is None:
            return
        start_y, start_first = self._drag
        _inset, height = self._track()
        top, bottom = self._thumb()
        # Pointer travel over the track's free length maps onto the view's.
        free = max(1.0, height - (bottom - top))
        hidden = 1.0 - (self._last - self._first)
        delta = (event.y - start_y) / free * hidden
        self._command("moveto", min(hidden, max(0.0, start_first + delta)))

    def _release(self, event) -> None:
        self._drag = None
        self.redraw()
