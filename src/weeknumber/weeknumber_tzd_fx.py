"""Small, quiet animations for the Timezone Dominator board and the dice.

Everything here draws canvas items tagged "fx" over the board, which
TzdBoardView keeps across re-renders, so effects cost no bitmap work:

* arrows from attacker to target (live for your own selection, briefly
  for every attack and fortify, yours or not);
* a ring that spreads from a conquered territory and a bump on a badge
  that gained armies;
* the armies lost, floating up from both badges;
* a turn announcement that drops in over the board in the player's colour,
  and a caption along the board's foot saying what just happened;
* the victory: a ring wave across the board, falling confetti and a banner.

Tk's canvas has no transparency, so a fade blends an item's colour into the
map behind it. With the operating system's "reduce motion" setting on,
effects appear and disappear without moving.

DiceRow draws the last roll as dice with pips, attacker against defender,
the dice that lost their comparison dimmed, after a short cosmetic roll.
CardHand draws the player's cards as small playing cards.
"""

from __future__ import annotations

import math
import random
import tkinter as tk
import tkinter.font as tkfont
from typing import TYPE_CHECKING

from PIL import Image, ImageDraw, ImageFilter, ImageTk

from weeknumber_widgets import blend, rounded

if TYPE_CHECKING:
    from collections.abc import Callable

    from weeknumber_tzd_board import TzdBoardView


def _luminance(color: str) -> float:
    """0 for black to 1 for white, from a #RRGGBB colour."""
    r, g, b = (int(color[i:i + 2], 16) / 255 for i in (1, 3, 5))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def _smooth(t: float) -> float:
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


class _Animation:
    def __init__(self, duration_ms: int, step: "Callable[[float], None]",
                 done: "Callable[[], None] | None" = None) -> None:
        self.duration_ms = max(1, duration_ms)
        self.step = step
        self.done = done
        self.elapsed = 0


class BoardEffects:
    """Effects over one TzdBoardView. Call `cancel()` when the window closes."""

    FRAME_MS = 30
    TAG = "fx"

    def __init__(self, view: "TzdBoardView", theme: dict[str, str], reduce_motion: bool) -> None:
        self.view = view
        self.canvas = view.canvas
        self.theme = theme
        self.reduce_motion = reduce_motion
        self._animations: list[_Animation] = []
        self._after: "str | None" = None
        self._serial = 0
        self._banner_image = None
        self._announce_image = None
        self._announce_after: "str | None" = None
        self._toast_image = None
        self._toast_after: "str | None" = None

    # -- engine ---------------------------------------------------------------
    def _px(self, value: float) -> int:
        return self.view._px(value)

    def _new_tag(self) -> str:
        self._serial += 1
        return f"fx{self._serial}"

    def _run(self, duration_ms: int, step, done=None) -> None:
        if self.reduce_motion:
            step(1.0)
            if done is not None:
                self.canvas.after(duration_ms, done)
            return
        self._animations.append(_Animation(duration_ms, step, done))
        step(0.0)
        if self._after is None:
            self._after = self.canvas.after(self.FRAME_MS, self._tick)

    def _tick(self) -> None:
        self._after = None
        try:
            if not self.canvas.winfo_exists():
                return
        except tk.TclError:
            return
        for animation in list(self._animations):
            animation.elapsed += self.FRAME_MS
            t = animation.elapsed / animation.duration_ms
            animation.step(min(1.0, t))
            if t >= 1.0:
                self._animations.remove(animation)
                if animation.done is not None:
                    animation.done()
        if self._animations:
            self._after = self.canvas.after(self.FRAME_MS, self._tick)

    def cancel(self) -> None:
        for name in ("_after", "_announce_after", "_toast_after"):
            timer = getattr(self, name)
            if timer is not None:
                try:
                    self.canvas.after_cancel(timer)
                except tk.TclError:
                    pass
                setattr(self, name, None)
        self._animations.clear()

    def _fade(self, tag: str, color: str, t: float, option: str = "fill") -> None:
        """Blend an item's colour into the map behind it (t: 0 solid, 1 gone)."""
        self.canvas.itemconfigure(tag, **{option: blend(color, self.theme["MAP_OCEAN"], t)})

    def _delete(self, tag: str) -> "Callable[[], None]":
        return lambda: self.canvas.delete(tag)

    # -- arrows -----------------------------------------------------------------
    def _arrow_points(self, source: str, target: str) -> "list[list[float]]":
        """One curved segment, or two straight ones across the map's edge for
        the Alaska-Kamchatka link (the map wraps there)."""
        (x0, y0), (x1, y1) = self.view.label_xy(source), self.view.label_xy(target)
        radius = self._px(self.view.BADGE_PX) + self._px(3)
        if abs(x1 - x0) > self.view.width / 2:
            edge = 0 if x0 < x1 else self.view.width
            mid_y = (y0 + y1) / 2
            return [[x0, y0, edge, mid_y], [self.view.width - edge, mid_y, x1, y1]]
        dx, dy = x1 - x0, y1 - y0
        length = math.hypot(dx, dy) or 1.0
        ux, uy = dx / length, dy / length
        start = (x0 + ux * radius, y0 + uy * radius)
        end = (x1 - ux * radius, y1 - uy * radius)
        bend = min(length * 0.18, self._px(40))
        mid = ((x0 + x1) / 2 - uy * bend, (y0 + y1) / 2 + ux * bend)
        return [[*start, *mid, *end]]

    def arrow(self, source: str, target: str, color: str, key: "str | None" = None,
              hold_ms: int = 900) -> None:
        """An arrow source -> target. With `key` it stays until replaced or
        `clear(key)`; without, it shows for `hold_ms` and fades."""
        if key is not None:
            self.clear(key)
        tag = f"fx-{key}" if key is not None else self._new_tag()
        width = self._px(3)
        shape = (self._px(10), self._px(12), self._px(4))
        segments = self._arrow_points(source, target)
        for index, points in enumerate(segments):
            self.canvas.create_line(
                *points, fill=color, width=width, smooth=len(points) > 4,
                arrow="last" if index == len(segments) - 1 else "none",
                arrowshape=shape, capstyle="round", joinstyle="round",
                tags=(self.TAG, tag))
        self.canvas.tag_raise(self.TAG)
        if key is None:
            fade_ms = 400
            self.canvas.after(hold_ms, lambda: self._run(
                fade_ms, lambda t: self._fade(tag, color, t), self._delete(tag)))

    def clear(self, key: str) -> None:
        self.canvas.delete(f"fx-{key}")

    # -- pulses -------------------------------------------------------------------
    def ring(self, tid: str, color: str, delay_ms: int = 0, grow: float = 3.2,
             duration_ms: int = 520) -> None:
        """A ring that spreads from a territory's badge and fades."""
        def start() -> None:
            x, y = self.view.label_xy(tid)
            r0 = self._px(self.view.BADGE_PX)
            tag = self._new_tag()
            item = self.canvas.create_oval(x - r0, y - r0, x + r0, y + r0, outline=color,
                                           width=self._px(2.5), tags=(self.TAG, tag))

            def step(t: float) -> None:
                r = r0 * (1 + (grow - 1) * _smooth(t))
                self.canvas.coords(item, x - r, y - r, x + r, y + r)
                self._fade(tag, color, _smooth(t), "outline")

            if self.reduce_motion:
                self.canvas.after(duration_ms, self._delete(tag))
            else:
                self._run(duration_ms, step, self._delete(tag))

        if delay_ms:
            self.canvas.after(delay_ms, start)
        else:
            start()

    def bump(self, tid: str, color: str) -> None:
        """A short tight pulse on a badge that gained armies."""
        self.ring(tid, color, grow=1.8, duration_ms=360)

    def float_text(self, tid: str, text: str, color: str) -> None:
        """A number rising from a badge, e.g. armies lost."""
        if self.reduce_motion:
            return
        x, y = self.view.label_xy(tid)
        tag = self._new_tag()
        item = self.canvas.create_text(
            x + self._px(14), y - self._px(6), text=text, fill=color,
            font=(self.view.family, -self._px(12), "bold"), tags=(self.TAG, tag))
        rise = self._px(22)

        def step(t: float) -> None:
            self.canvas.coords(item, x + self._px(14), y - self._px(6) - rise * _smooth(t))
            self._fade(tag, color, max(0.0, (t - 0.45) / 0.55))

        self._run(900, step, self._delete(tag))

    # -- what is happening ---------------------------------------------------------
    def _card_image(self, width: int, height: int, radius: float, fill: str,
                    shadow: bool = True) -> "ImageTk.PhotoImage":
        """A capsule with a soft shadow, transparent around it, so it sits on
        the map like a sheet of paper."""
        pad = self._px(10) if shadow else 0
        ss = 3
        big = Image.new("RGBA", ((width + 2 * pad) * ss, (height + 2 * pad) * ss), (0, 0, 0, 0))
        if shadow:
            shade = Image.new("RGBA", big.size, (0, 0, 0, 0))
            ImageDraw.Draw(shade).rounded_rectangle(
                (pad * ss, (pad + self._px(3)) * ss, (pad + width) * ss, (pad + height + self._px(3)) * ss),
                radius=radius * ss, fill=(0, 0, 0, 70))
            big = Image.alpha_composite(big, shade.filter(ImageFilter.GaussianBlur(self._px(5) * ss)))
        ImageDraw.Draw(big).rounded_rectangle(
            (pad * ss, pad * ss, (pad + width) * ss - 1, (pad + height) * ss - 1),
            radius=radius * ss, fill=fill)
        image = big.resize((width + 2 * pad, height + 2 * pad), Image.LANCZOS)
        return ImageTk.PhotoImage(image, master=self.canvas)

    def announce(self, title: str, subtitle: str, fill: str, ink: str,
                 hold_ms: int = 1500) -> None:
        """Whose turn it is, in large type on a capsule of their colour that
        drops in over the top of the board, stays a moment and lifts away."""
        self.clear_announcement()
        family = self.view.family
        title_font = (family, -self._px(22), "bold")
        sub_font = (family, -self._px(12), "bold")
        measure = tkfont.Font(root=self.canvas, font=title_font)
        sub_width = tkfont.Font(root=self.canvas, font=sub_font).measure(subtitle)
        width = min(self.view.width - self._px(40),
                    max(self._px(240), measure.measure(title) + self._px(56),
                        sub_width + self._px(56)))
        height = self._px(70 if subtitle else 50)
        self._announce_image = self._card_image(width, height, height / 2.0, fill)
        cx = self.view.width / 2
        rest_y = self.view.legend_height + self._px(18) + height / 2
        tag = "fx-announce"
        image = self.canvas.create_image(cx, rest_y, image=self._announce_image,
                                         tags=(self.TAG, tag))
        title_y = rest_y - (self._px(10) if subtitle else 0)
        items = [image, self.canvas.create_text(cx, title_y, text=title, fill=ink,
                                                font=title_font, tags=(self.TAG, tag))]
        if subtitle:
            items.append(self.canvas.create_text(cx, rest_y + self._px(16), text=subtitle,
                                                 fill=blend(ink, fill, 0.18), font=sub_font,
                                                 tags=(self.TAG, tag)))
        self.canvas.tag_raise(tag)
        drop = self._px(16)
        if not self.reduce_motion:
            self.canvas.move(tag, 0, -drop)
            moved = [0.0]

            def slide_in(t: float) -> None:
                target = drop * _smooth(t)
                self.canvas.move(tag, 0, target - moved[0])
                moved[0] = target

            self._run(220, slide_in)

        def leave() -> None:
            self._announce_after = None
            if self.reduce_motion:
                self.clear_announcement()
                return
            lifted = [0.0]

            def slide_out(t: float) -> None:
                target = -drop * _smooth(t)
                self.canvas.move(tag, 0, target - lifted[0])
                lifted[0] = target
                for item in items[1:]:
                    self.canvas.itemconfigure(item, fill=blend(ink, fill, _smooth(t)))

            self._run(260, slide_out, self.clear_announcement)

        self._announce_after = self.canvas.after(hold_ms, leave)

    def clear_announcement(self) -> None:
        if self._announce_after is not None:
            try:
                self.canvas.after_cancel(self._announce_after)
            except tk.TclError:
                pass
            self._announce_after = None
        self.canvas.delete("fx-announce")
        self._announce_image = None

    def toast(self, runs: "list[tuple[str, str]]", hold_ms: int = 2800) -> None:
        """A one-line caption along the foot of the board: what just
        happened, names in their players' colours. A new one replaces it."""
        self.clear_toast()
        if not runs:
            return
        family = self.view.family
        font = (family, -self._px(12))
        bold = (family, -self._px(12), "bold")
        plain = tkfont.Font(root=self.canvas, font=font)
        heavy = tkfont.Font(root=self.canvas, font=bold)
        theme = self.theme
        default_ink = theme["TITLE_FG"]
        widths = [(heavy if color != default_ink else plain).measure(text) for text, color in runs]
        pad = self._px(16)
        limit = self.view.width - self._px(48)
        width = min(limit, sum(widths) + 2 * pad)
        height = self._px(30)
        fill = theme["CARD_BG"]
        self._toast_image = self._card_image(width, height, height / 2.0, fill)
        cx = self.view.width / 2
        cy = self.view.legend_height + self.view.height - self._px(14) - height / 2
        tag = "fx-toast"
        self.canvas.create_image(cx, cy, image=self._toast_image, tags=(self.TAG, tag))
        x = cx - width / 2 + pad
        right = cx + width / 2 - pad
        for (text, color), w in zip(runs, widths):
            if x >= right:
                break
            measure = heavy if color != default_ink else plain
            if x + w > right:  # longer than the board: cut it with an ellipsis
                while text and x + measure.measure(text + "\u2026") > right:
                    text = text[:-1]
                text += "\u2026"
            self.canvas.create_text(x, cy, text=text, anchor="w", fill=color,
                                    font=bold if color != default_ink else font,
                                    tags=(self.TAG, tag))
            x += w
        self.canvas.tag_raise(tag)
        self.canvas.tag_raise("fx-announce")
        self._toast_after = self.canvas.after(hold_ms, self.clear_toast)

    def clear_toast(self) -> None:
        if self._toast_after is not None:
            try:
                self.canvas.after_cancel(self._toast_after)
            except tk.TclError:
                pass
            self._toast_after = None
        self.canvas.delete("fx-toast")
        self._toast_image = None

    # -- victory --------------------------------------------------------------------
    def celebrate(self, order: "list[str]", winner_color: str, colors: "list[str]",
                  title: str, subtitle: str, title_fill: str, title_color: str,
                  festive: bool) -> None:
        """A ring wave across the board in the winner's colour, confetti when
        `festive`, then a banner that stays."""
        step_ms = 1200 // max(1, len(order))
        for index, tid in enumerate(order):
            self.ring(tid, winner_color, delay_ms=index * step_ms, grow=2.6)
        if festive and not self.reduce_motion:
            self.confetti(colors)
        self.canvas.after(0 if self.reduce_motion else 500,
                          lambda: self.banner(title, subtitle, title_fill, title_color))

    def confetti(self, colors: "list[str]", pieces: int = 80, duration_ms: int = 3200) -> None:
        rng = random.Random()
        width, top = self.view.width, self.view.legend_height
        height = self.view.height
        flakes = []
        for _ in range(pieces):
            x = rng.uniform(0, width)
            size = self._px(rng.uniform(4, 7))
            tag = self._new_tag()
            color = rng.choice(colors)
            item = self.canvas.create_oval(x, top - size, x + size, top + size * 0.6,
                                           fill=color, outline="", tags=(self.TAG, tag))
            flakes.append((item, tag, color, x, size, rng.uniform(0.0, 0.35),
                           rng.uniform(0.75, 1.15), rng.uniform(0, math.tau)))

        def step(t: float) -> None:
            for item, tag, color, x, size, delay, speed, phase in flakes:
                local = max(0.0, (t - delay) / (1 - delay))
                y = top - size + (height + size) * local * speed
                sway = self._px(10) * math.sin(phase + local * 9)
                self.canvas.coords(item, x + sway, y, x + sway + size, y + size * 0.6)
                if local > 0.8:
                    self._fade(tag, color, (local - 0.8) / 0.2)

        def done() -> None:
            for item, tag, *_ in flakes:
                self.canvas.delete(tag)

        self._run(duration_ms, step, done)

    def banner(self, title: str, subtitle: str, fill: str, color: str) -> None:
        """A large capsule over the middle of the board; it stays. It widens
        for a long winner's name, up to the board, and only past that does
        the title get smaller."""
        self.canvas.delete("fx-banner")
        family = self.view.family
        pad = self._px(28)
        widest = max(self._px(340), self.view.width - self._px(32))
        sub_font = (family, -self._px(12))
        sub_width = tkfont.Font(root=self.canvas, font=sub_font).measure(subtitle)
        for size in range(26, 12, -2):
            title_font = (family, -self._px(size), "bold")
            title_width = tkfont.Font(root=self.canvas, font=title_font).measure(title)
            if title_width + 2 * pad <= widest:
                break
        width = min(widest, max(self._px(340), title_width + 2 * pad, sub_width + 2 * pad))
        height = self._px(92)
        cx = self.view.width / 2
        cy = self.view.legend_height + self.view.height / 2
        card = rounded(width, height, self._px(26), fill, None)
        self._banner_image = ImageTk.PhotoImage(card, master=self.canvas)
        self.canvas.create_image(cx, cy, image=self._banner_image, tags=(self.TAG, "fx-banner"))
        self.canvas.create_text(cx, cy - self._px(13), text=title, fill=color,
                                font=title_font, tags=(self.TAG, "fx-banner"))
        self.canvas.create_text(cx, cy + self._px(21), text=subtitle,
                                fill=blend(color, fill, 0.25),
                                font=sub_font, tags=(self.TAG, "fx-banner"))
        self.canvas.tag_raise("fx-banner")


class DiceRow(tk.Canvas):
    """The last roll as dice: attacker's in their colour, defender's in
    theirs, a die that lost its comparison dimmed."""

    DIE_PX = 22
    GAP_PX = 5
    ROLL_FRAMES = 7
    ROLL_FRAME_MS = 40

    def __init__(self, master: tk.Misc, scale: float, family: str) -> None:
        super().__init__(master, width=1, height=1, bd=0, highlightthickness=0)
        self._scale = scale
        self._family = family
        self._images: dict[tuple, ImageTk.PhotoImage] = {}
        self._theme: dict[str, str] = {}
        self._after: "str | None" = None
        self._shown = None

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def apply_theme(self, theme: dict[str, str]) -> None:
        self._theme = theme
        self._images.clear()
        self.configure(bg=theme["CARD_BG"])
        if self._shown is not None:
            self._draw(*self._shown)

    def _die(self, value: int, color: str, dim: bool) -> ImageTk.PhotoImage:
        theme = self._theme
        key = (value, color, dim)
        image = self._images.get(key)
        if image is not None:
            return image
        size, ss = self._px(self.DIE_PX), 4
        fill = blend(color, theme["CARD_BG"], 0.62) if dim else color
        pip = blend("#FFFFFF", fill, 0.25) if dim else "#FFFFFF"
        big = rounded(size * ss, size * ss, size * ss * 0.24, fill, theme["CARD_BG"], supersample=1)
        draw = ImageDraw.Draw(big)
        r = size * ss * 0.095
        spots = {1: [(0.5, 0.5)], 2: [(0.27, 0.27), (0.73, 0.73)],
                 3: [(0.27, 0.27), (0.5, 0.5), (0.73, 0.73)],
                 4: [(0.27, 0.27), (0.73, 0.27), (0.27, 0.73), (0.73, 0.73)],
                 5: [(0.27, 0.27), (0.73, 0.27), (0.5, 0.5), (0.27, 0.73), (0.73, 0.73)],
                 6: [(0.27, 0.25), (0.73, 0.25), (0.27, 0.5), (0.73, 0.5), (0.27, 0.75), (0.73, 0.75)]}
        for fx, fy in spots.get(value, []):
            cx, cy = fx * size * ss, fy * size * ss
            draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=pip)
        image = ImageTk.PhotoImage(big.resize((size, size), Image.LANCZOS), master=self)
        self._images[key] = image
        return image

    def show(self, attack: "list[int]", defence: "list[int]", attacker: str, defender: str,
             roll: bool = True) -> None:
        """Draw a roll. Dice that lost are dimmed: per pair, the higher die
        wins and the defender wins ties."""
        if self._after is not None:
            self.after_cancel(self._after)
            self._after = None
        lost_att = [False] * len(attack)
        lost_def = [False] * len(defence)
        for i in range(min(len(attack), len(defence))):
            if attack[i] > defence[i]:
                lost_def[i] = True
            else:
                lost_att[i] = True
        final = (attack, defence, attacker, defender, lost_att, lost_def)
        if not roll:
            self._shown = final
            self._draw(*final)
            return
        rng = random.Random()

        def frame(n: int) -> None:
            self._after = None
            if n >= self.ROLL_FRAMES:
                self._shown = final
                self._draw(*final)
                return
            self._draw([rng.randint(1, 6) for _ in attack], [rng.randint(1, 6) for _ in defence],
                       attacker, defender, [False] * len(attack), [False] * len(defence))
            self._after = self.after(self.ROLL_FRAME_MS, frame, n + 1)

        frame(0)

    def clear(self) -> None:
        if self._after is not None:
            self.after_cancel(self._after)
            self._after = None
        self._shown = None
        self.delete("all")
        self.configure(width=1, height=1)

    def _draw(self, attack, defence, attacker, defender, lost_att, lost_def) -> None:
        size, gap = self._px(self.DIE_PX), self._px(self.GAP_PX)
        versus = self._px(26)
        width = (len(attack) + len(defence)) * (size + gap) + versus
        self.delete("all")
        self.configure(width=width, height=size)
        x = 0
        for value, lost in zip(attack, lost_att):
            self.create_image(x, 0, anchor="nw", image=self._die(value, attacker, lost))
            x += size + gap
        self.create_text(x + versus / 2 - gap / 2, size / 2, text="vs",
                         fill=self._theme.get("YEAR_FG", "#888888"),
                         font=(self._family, -self._px(10), "bold"))
        x += versus
        for value, lost in zip(defence, lost_def):
            self.create_image(x, 0, anchor="nw", image=self._die(value, defender, lost))
            x += size + gap


class CardHand(tk.Canvas):
    """Your cards as small playing cards. Each has a coloured header with its
    symbol in white -- infantry a soldier, cavalry a horseshoe, artillery a
    cannon, wild a star -- the territory's name on the face and the symbol's
    name in its colour at the foot, so a set (three alike, or one of each)
    is easy to spot. `lifted` cards -- the set the Trade button will use --
    sit raised with an accent edge and a check. The cards only show: which
    set is traded is the rules' choice (weeknumber_tzd_rules.find_set)."""

    CARD_W_PX = 52
    CARD_H_PX = 74
    GAP_PX = 7
    LIFT_PX = 7
    MAX_WIDTH_PX = 340
    #: Room around each card for its shadow, px @ 96 DPI.
    SHADOW_PX = 4
    SYMBOL_NAMES = {"infantry": "INFANTRY", "cavalry": "CAVALRY",
                    "artillery": "ARTILLERY", "wild": "WILD"}

    def __init__(self, master: tk.Misc, scale: float, family: str) -> None:
        super().__init__(master, width=1, height=1, bd=0, highlightthickness=0)
        self._scale = scale
        self._family = family
        self._theme: dict[str, str] = {}
        self._images: dict[tuple, ImageTk.PhotoImage] = {}
        self._shown = None

    def _px(self, value: float) -> int:
        return max(1, round(value * self._scale))

    def apply_theme(self, theme: dict[str, str]) -> None:
        self._theme = theme
        self._images.clear()
        self.configure(bg=theme["CARD_BG"])
        shown, self._shown = self._shown, None
        if shown is not None:
            self.show(*shown)

    def symbol_color(self, symbol: str) -> str:
        key = {"infantry": "TZD_CARD_INFANTRY", "cavalry": "TZD_CARD_CAVALRY",
               "artillery": "TZD_CARD_ARTILLERY"}.get(symbol, "TZD_CARD_WILD")
        return self._theme[key]

    @staticmethod
    def _glyph(draw: "ImageDraw.ImageDraw", symbol: str, cx: float, cy: float, r: float,
               ink: str) -> None:
        """The symbol, drawn in `ink` inside a circle of radius `r`."""
        if symbol == "infantry":  # a soldier: helmeted head over shoulders
            head = r * 0.34
            draw.ellipse((cx - head, cy - r * 0.78, cx + head, cy - r * 0.78 + 2 * head), fill=ink)
            draw.pieslice((cx - r * 0.42, cy - r * 0.9, cx + r * 0.42, cy - r * 0.2),
                          180, 360, fill=ink)                       # the helmet's brim
            draw.rounded_rectangle((cx - r * 0.72, cy + r * 0.02, cx + r * 0.72, cy + r * 0.86),
                                   radius=r * 0.36, fill=ink)
        elif symbol == "cavalry":  # a horseshoe, open side down
            outer, inner = r * 0.82, r * 0.42
            draw.pieslice((cx - outer, cy - outer, cx + outer, cy + outer), 180, 360, fill=ink)
            draw.rectangle((cx - outer, cy, cx - inner, cy + r * 0.78), fill=ink)
            draw.rectangle((cx + inner, cy, cx + outer, cy + r * 0.78), fill=ink)
            draw.pieslice((cx - inner, cy - inner, cx + inner, cy + inner), 180, 360,
                          fill=(0, 0, 0, 0))
            nail = r * 0.09
            for fx, fy in ((-0.62, -0.18), (0.62, -0.18), (-0.62, 0.38), (0.62, 0.38)):
                x, y = cx + fx * r, cy + fy * r
                draw.ellipse((x - nail, y - nail, x + nail, y + nail), fill=(0, 0, 0, 0))
        elif symbol == "artillery":  # a cannon: barrel raised over its wheel
            angle = math.radians(-26)
            length, half = r * 1.55, r * 0.25
            bx, by = cx - r * 0.7, cy + r * 0.3
            dx, dy = math.cos(angle), math.sin(angle)
            nx, ny = -dy, dx
            tip_x, tip_y = bx + dx * length, by + dy * length
            draw.polygon([(bx + nx * half * 1.25, by + ny * half * 1.25),
                          (tip_x + nx * half, tip_y + ny * half),
                          (tip_x - nx * half, tip_y - ny * half),
                          (bx - nx * half * 1.25, by - ny * half * 1.25)], fill=ink)
            wheel = r * 0.5
            wx, wy = cx - r * 0.2, cy + r * 0.45
            draw.ellipse((wx - wheel, wy - wheel, wx + wheel, wy + wheel), fill=ink)
            hub = wheel * 0.38
            draw.ellipse((wx - hub, wy - hub, wx + hub, wy + hub), fill=(0, 0, 0, 0))
        else:  # wild: a five-pointed star
            points = []
            for i in range(10):
                a = -math.pi / 2 + i * math.pi / 5
                radius_i = r * (0.95 if i % 2 == 0 else 0.4)
                points.append((cx + radius_i * math.cos(a), cy + radius_i * math.sin(a)))
            draw.polygon(points, fill=ink)

    def _card(self, symbol: str, lifted: bool) -> ImageTk.PhotoImage:
        key = (symbol, lifted)
        image = self._images.get(key)
        if image is not None:
            return image
        theme = self._theme
        w, h, ss = self._px(self.CARD_W_PX), self._px(self.CARD_H_PX), 4
        pad = self._px(self.SHADOW_PX)
        dark = _luminance(theme["CARD_BG"]) < 0.5
        face = blend(theme["CARD_BG"], "#FFFFFF", 0.09) if dark else "#FFFFFF"
        color = self.symbol_color(symbol)
        radius = self._px(7) * ss
        box = (pad * ss, pad * ss, (pad + w) * ss - 1, (pad + h) * ss - 1)
        size = ((w + 2 * pad) * ss, (h + 2 * pad) * ss)

        shadow = Image.new("RGBA", size, (0, 0, 0, 0))
        lift = self._px(3 if lifted else 1.5) * ss
        ImageDraw.Draw(shadow).rounded_rectangle(
            (box[0], box[1] + lift, box[2], box[3] + lift), radius=radius,
            fill=(0, 0, 0, 120 if dark else 55))
        big = shadow.filter(ImageFilter.GaussianBlur(self._px(2.5) * ss))

        card = Image.new("RGBA", size, (0, 0, 0, 0))
        draw = ImageDraw.Draw(card)
        draw.rounded_rectangle(box, radius=radius, fill=face)
        header = Image.new("RGBA", size, (0, 0, 0, 0))
        band = box[1] + round(h * 0.42) * ss
        ImageDraw.Draw(header).rectangle((box[0], box[1], box[2], band), fill=color)
        clip = Image.new("L", size, 0)
        ImageDraw.Draw(clip).rounded_rectangle(box, radius=radius, fill=255)
        card.alpha_composite(Image.composite(header, Image.new("RGBA", size, (0, 0, 0, 0)), clip))
        glyph = Image.new("RGBA", size, (0, 0, 0, 0))
        self._glyph(ImageDraw.Draw(glyph), symbol, (box[0] + box[2]) / 2,
                    (box[1] + band) / 2 + ss, round(h * 0.15) * ss, "#FFFFFF")
        card.alpha_composite(glyph)
        edge = theme["ACCENT"] if lifted else theme["CARD_BORDER"]
        ImageDraw.Draw(card).rounded_rectangle(
            box, radius=radius, outline=edge,
            width=max(1, round((2.2 if lifted else 1) * self._scale * ss)))
        if lifted:  # a check badge on the corner
            r = self._px(7) * ss
            cx, cy = box[2] - r * 0.55, box[1] + r * 0.55
            badge = ImageDraw.Draw(card)
            badge.ellipse((cx - r, cy - r, cx + r, cy + r), fill=theme["ACCENT"],
                          outline=face, width=max(1, round(1.5 * self._scale * ss)))
            stroke = max(1, round(1.8 * self._scale * ss))
            badge.line([(cx - r * 0.45, cy + r * 0.02), (cx - r * 0.1, cy + r * 0.38),
                        (cx + r * 0.5, cy - r * 0.35)], fill="#FFFFFF", width=stroke, joint="curve")
        big.alpha_composite(card)
        image = ImageTk.PhotoImage(big.resize((w + 2 * pad, h + 2 * pad), Image.LANCZOS),
                                   master=self)
        self._images[key] = image
        return image

    def show(self, cards: "list[tuple[str, str]]", lifted: "set[int]") -> None:
        """`cards` are (symbol, label) pairs in hand order."""
        key = (tuple(cards), frozenset(lifted))
        if key == self._shown:
            return
        self._shown = key
        self._draw()

    def _draw(self) -> None:
        self.delete("all")
        if self._shown is None or not self._theme:
            return  # apply_theme draws it
        cards, lifted = self._shown
        if not cards:
            self.configure(width=1, height=1)
            return
        w, h = self._px(self.CARD_W_PX), self._px(self.CARD_H_PX)
        pad, lift = self._px(self.SHADOW_PX), self._px(self.LIFT_PX)
        step = min(w + self._px(self.GAP_PX),
                   (self._px(self.MAX_WIDTH_PX) - w) / max(1, len(cards) - 1))
        width = int(step * (len(cards) - 1) + w + 2 * pad)
        self.configure(width=width, height=h + 2 * pad + lift)
        name_font = (self._family, -self._px(8.5), "bold")
        kind_font = (self._family, -self._px(7), "bold")
        for index, (symbol, label) in enumerate(cards):
            raised = index in lifted
            if symbol == "wild":
                label = "Any symbol"  # the foot already says WILD
            x = index * step
            y = 0 if raised else lift
            self.create_image(x, y, anchor="nw", image=self._card(symbol, raised))
            self.create_text(x + pad + w / 2, y + pad + h * 0.42 + h * 0.58 * 0.42,
                             text=label, fill=self._theme["TITLE_FG"], font=name_font,
                             width=w - self._px(6), justify="center")
            self.create_text(x + pad + w / 2, y + pad + h - self._px(8),
                             text=self.SYMBOL_NAMES.get(symbol, symbol.upper()),
                             fill=self.symbol_color(symbol), font=kind_font)
