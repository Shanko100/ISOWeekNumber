"""The Timezone Dominator board: the time-zone map, repainted by territory owner.

Everything below the territories is the time-zone picker's own drawing from
WorldMapRenderer -- the same projection, the ocean washed with the band
tints, the same supersampling -- so the board reads as part of the app rather
than a separate game. On top of it:

* each territory is filled with its owner's colour, washed toward the ocean
  so owned land sits at the band colours' lightness, and outlined with the
  map's border hairline;
* sea routes are dashed, antialiased lines;
* army counts are badges in the owner's saturated colour, their numerals
  kept as canvas text so they stay crisp, exactly like the offset ruler;
* every territory carries an inner outline in its continent's colour, so
  which continent a country belongs to reads straight off the map;
* the strip along the top is the continent legend, in the ruler's style and
  in the same continent colours: a continent held by one player takes that
  player's colour, with its own colour kept as a bar along the cell's foot;
* the frame around the map is the colour of the player whose turn it is.

The selection uses the time-zone map's highlight colour: a heavy outline on
the chosen territory and a tinted wash over every legal target.
"""

from __future__ import annotations

import tkinter as tk
from typing import TYPE_CHECKING

from PIL import Image, ImageChops, ImageDraw

import weeknumber_tzd_rules as rules
from weeknumber_map import WorldMapRenderer
from weeknumber_timezone import _point_in_ring
from weeknumber_widgets import blend

if TYPE_CHECKING:
    from collections.abc import Callable


def _ring_bounds(ring: list[float]) -> "tuple[float, float, float, float]":
    """(west, east, south, north) of a flat [lon, lat, ...] ring."""
    lons, lats = ring[0::2], ring[1::2]
    return min(lons), max(lons), min(lats), max(lats)


def player_color(theme: dict[str, str], color_slot: int) -> str:
    if 0 <= color_slot < rules.MAX_PLAYERS:
        return theme[f"TZD_PLAYER_{color_slot}"]
    return theme["TZD_NEUTRAL"]


def text_on(color: str) -> str:
    """Black or white, whichever reads better on `color`."""
    value = color.lstrip("#")
    r, g, b = (int(value[i:i + 2], 16) for i in (0, 2, 4))
    return "#1F2937" if (0.299 * r + 0.587 * g + 0.114 * b) > 170 else "#FFFFFF"


#: The themes define this many continent colours (TZD_CONTINENT_n).
CONTINENT_COLORS = 6


def continent_color(theme: dict[str, str], index: int) -> str:
    """The continent at `index` in board order, in its own colour: the inner
    outline of its territories and the hue of its legend cell."""
    return theme[f"TZD_CONTINENT_{index % CONTINENT_COLORS}"]


def continent_tint(theme: dict[str, str], index: int) -> str:
    """A continent's colour washed toward the map: its legend cell, and its
    territories on the unplayed board."""
    return blend(continent_color(theme, index), theme["MAP_OCEAN"], 0.55)


def continent_fill(board: rules.Board, theme: dict[str, str], tid: str) -> str:
    """A territory in its continent's tint: the board before anyone owns
    anything, and the frame the time-zone map turns into."""
    continent = board.territories[tid].continent
    return continent_tint(theme, board.continent_order.index(continent))


def draw_routes(renderer: WorldMapRenderer, draw, routes, color: str,
                dash_px: float, gap_px: float) -> None:
    """Dashed sea crossings. Pillow has no dash style, so each route is cut
    into equal segments in the oversampled bitmap."""
    scale = renderer.supersample
    dash, gap = dash_px * scale, gap_px * scale
    width = max(1, round(scale * 1.1))
    for _a, _b, line in routes:
        (x0, y0), (x1, y1) = renderer.hi_poly(line)
        length = ((x1 - x0) ** 2 + (y1 - y0) ** 2) ** 0.5
        if length < 1:
            continue
        ux, uy = (x1 - x0) / length, (y1 - y0) / length
        position = 0.0
        while position < length:
            end = min(length, position + dash)
            draw.line((x0 + ux * position, y0 + uy * position,
                       x0 + ux * end, y0 + uy * end), fill=color, width=width)
            position = end + gap


def draw_continent_legend(canvas: tk.Canvas, board: rules.Board,
                          theme: dict[str, str], width: float, height: float,
                          font, fills: "dict[str, str] | None" = None,
                          tags: str = "legend") -> None:
    """Continent cells along the top in the offset ruler's style: each cell
    in its continent's tint with a bar of its outline colour along the foot,
    or in `fills[continent]` (bold) when one player holds the whole
    continent -- the bar stays, so the cell still says which outline it is."""
    fills = fills or {}
    cell = width / max(1, len(board.continent_order))
    bar = max(2, round(height * 0.16))
    for index, cid in enumerate(board.continent_order):
        continent = board.continents[cid]
        held = cid in fills
        fill = fills.get(cid, continent_tint(theme, index))
        x0, x1 = index * cell, (index + 1) * cell
        canvas.create_rectangle(x0, 0, x1, height, fill=fill, outline="", tags=tags)
        canvas.create_rectangle(x0, height - bar, x1, height,
                                fill=continent_color(theme, index), outline="", tags=tags)
        canvas.create_text((x0 + x1) / 2.0, (height - bar) / 2.0,
                           text=f"{continent.name}  +{continent.bonus}",
                           fill=text_on(fill), tags=tags,
                           font=font + ("bold",) if held else font)


def render_preview(board: rules.Board, theme: dict[str, str], width: int,
                   height: int) -> "Image.Image":
    """The unplayed board at display size: what the time-zone map crossfades
    into when Timezone Dominator is started from it."""
    renderer = WorldMapRenderer(theme)
    renderer.resize(width, height)
    image, draw = renderer.new_image()
    renderer.paint_band_seas(draw, mix=TzdBoardView.SEA_MIX)
    renderer.paint_band_land(draw, fill=theme["TZD_OFFBOARD_LAND"])
    for tid in board.order:
        renderer.paint_rings(draw, board.territories[tid].rings,
                             continent_fill(board, theme, tid),
                             outline=theme["MAP_BORDER_LINE"], width=renderer.hairline)
    draw_routes(renderer, draw, board.routes, theme["WEEK_NUM_FG"],
                5 * renderer.width / 980, 3.5 * renderer.width / 980)
    return renderer.downsample(image)


class TzdBoardView:
    """A canvas showing one board state. Owns no game logic: the game window
    passes in the state, the local player's selection and the legal targets,
    and gets territory clicks back."""

    LEGEND_PX = 20
    #: How far territory fills move from the player colour toward the ocean.
    LAND_WASH = 0.42
    #: Band tints over open water: fainter than the picker's, so the stripes
    #: stay the map's signature without competing with the players' colours.
    SEA_MIX = 0.86
    #: Badge radius and numeral size, px @ 96 DPI.
    BADGE_PX = 9.5
    BADGE_FONT_PX = 9.5
    HIT_SLOP_PX = 14

    #: Width of the continent outline inside each territory, px @ 96 DPI.
    CONTINENT_EDGE_PX = 2.2

    def __init__(self, master: tk.Misc, board: rules.Board, theme: dict[str, str],
                 scale: float, family: str,
                 on_click: "Callable[[str], None]",
                 on_hover: "Callable[[str | None], None] | None" = None) -> None:
        self.board = board
        self.theme = theme
        self.scale = scale
        self.family = family
        self._on_click = on_click
        self._on_hover = on_hover
        self._renderer = WorldMapRenderer(theme)
        self._base = None
        #: Territory outlines in bitmap coordinates, projected once per size.
        self._polys: "dict[str, list[list]] | None" = None
        #: Hit-test candidates: (tid, ring, west, east, south, north), so a
        #: mouse move tests a handful of rings instead of every point.
        self._hit_rings = [(tid, ring) + _ring_bounds(ring)
                           for tid in board.order for ring in board.territories[tid].rings]
        self._photo = None
        #: The continent outlines as (colour image, mask), cached per size
        #: and theme: they never change during a game.
        self._edges: "tuple[Image.Image, Image.Image] | None" = None
        self._frame: "str | None" = None
        self._frame_width = 3.0
        self._hover: "str | None" = None
        self._rendered: "tuple | None" = None
        self.legend_height = self._px(self.LEGEND_PX)
        self.border = tk.Frame(master)
        self.canvas = tk.Canvas(self.border, width=10, height=10, bd=0,
                                highlightthickness=0, cursor="hand2")
        self.canvas.pack(padx=1, pady=1)
        self.canvas.bind("<Button-1>", self._clicked)
        self.canvas.bind("<Motion>", self._moved)
        self.canvas.bind("<Leave>", lambda e: self._set_hover(None))

    def _px(self, value: float) -> int:
        return max(1, round(value * self.scale))

    # -- sizing & theme -------------------------------------------------------
    @property
    def width(self) -> int:
        return self._renderer.width

    @property
    def height(self) -> int:
        return self._renderer.height

    def resize(self, width: int, height: int) -> None:
        self._renderer.resize(width, height)
        self.canvas.configure(width=width, height=height + self.legend_height)
        self._base = None
        self._polys = None
        self._edges = None

    def apply_theme(self, theme: dict[str, str]) -> None:
        self.theme = theme
        self._renderer.theme = theme
        self.border.configure(bg=theme["CARD_BORDER"])
        self.canvas.configure(bg=theme["CARD_BG"])
        self._base = None
        self._edges = None

    def _base_image(self):
        """Ocean, band seas and off-board land: unchanged by play, cached."""
        if self._base is None:
            image, draw = self._renderer.new_image()
            self._renderer.paint_band_seas(draw, mix=self.SEA_MIX)
            self._renderer.paint_band_land(draw, fill=self.theme["TZD_OFFBOARD_LAND"])
            self._base = image
        return self._base

    def _territory_polys(self) -> "dict[str, list[list]]":
        if self._polys is None:
            hi_poly = self._renderer.hi_poly
            self._polys = {tid: [hi_poly(ring) for ring in self.board.territories[tid].rings]
                           for tid in self.board.order}
        return self._polys

    def _continent_edges(self) -> "tuple[Image.Image, Image.Image]":
        """Every territory's inner outline in its continent's colour, as a
        colour image and the mask to paste it with.

        The stroke is clipped to its own territory, so where two continents
        meet each side keeps its own colour. Worked out once per size and
        theme, territory by territory inside its bounding box, so it costs a
        few tens of milliseconds when the window opens and nothing after."""
        if self._edges is not None:
            return self._edges
        renderer = self._renderer
        scale = renderer.supersample
        size = (max(1, renderer.width * scale), max(1, renderer.height * scale))
        colors = Image.new("RGB", size, "#000000")
        mask = Image.new("L", size, 0)
        width = max(2, round(self.CONTINENT_EDGE_PX * self.scale * scale))
        polys = self._territory_polys()
        for tid in self.board.order:
            rings = [points for points in polys[tid] if len(points) >= 3]
            if not rings:
                continue
            xs = [x for points in rings for x, _y in points]
            ys = [y for points in rings for _x, y in points]
            left = max(0, int(min(xs)) - width - 1)
            top = max(0, int(min(ys)) - width - 1)
            right = min(size[0], int(max(xs)) + width + 2)
            bottom = min(size[1], int(max(ys)) + width + 2)
            if right <= left or bottom <= top:
                continue
            box = (right - left, bottom - top)
            shape = Image.new("L", box, 0)
            stroke = Image.new("L", box, 0)
            shape_draw, stroke_draw = ImageDraw.Draw(shape), ImageDraw.Draw(stroke)
            for points in rings:
                local = [(x - left, y - top) for x, y in points]
                shape_draw.polygon(local, fill=255)
                stroke_draw.line(local + [local[0]], fill=255, width=2 * width, joint="curve")
            edge = ImageChops.multiply(shape, stroke)  # the half inside the territory
            continent = self.board.territories[tid].continent
            color = continent_color(self.theme, self.board.continent_order.index(continent))
            region = (left, top, right, bottom)
            colors.paste(Image.new("RGB", box, color), region, edge)
            mask.paste(ImageChops.lighter(mask.crop(region), edge), region)
        self._edges = (colors, mask)
        return self._edges

    def land_fill(self, color_slot: int) -> str:
        return blend(player_color(self.theme, color_slot), self.theme["MAP_OCEAN"],
                     self.LAND_WASH)

    # -- drawing --------------------------------------------------------------
    def render(self, state: "rules.GameState | None", selected: "str | None" = None,
               targets: "set[str] | None" = None) -> None:
        # Panel-only changes (the army stepper) re-request the same picture;
        # the supersampled board is too costly to redraw for nothing.
        board_key = None if state is None else (
            tuple(state.owner[t] for t in self.board.order),
            tuple(state.armies[t] for t in self.board.order),
            tuple(p["color"] for p in state.players))
        key = (board_key, selected, frozenset(targets or ()))
        if self._base is not None and key == self._rendered:
            return
        self._rendered = key
        renderer = self._renderer
        scale = renderer.supersample
        image = self._base_image().copy()
        draw = ImageDraw.Draw(image)
        theme = self.theme
        targets = targets or set()
        colors = {p["id"]: p["color"] for p in state.players} if state else {}
        highlight = theme["MAP_SELECT_LAND"]
        hairline = renderer.hairline
        polys = self._territory_polys()

        for tid in self.board.order:
            if state is not None:
                fill = self.land_fill(colors.get(state.owner[tid], -1))
            else:
                fill = continent_fill(self.board, theme, tid)
            if tid in targets:
                fill = blend(fill, highlight, 0.45)
            renderer.paint_polys(draw, polys[tid], fill,
                                 outline=theme["MAP_BORDER_LINE"], width=hairline)
        edges, edge_mask = self._continent_edges()
        image.paste(edges, (0, 0), edge_mask)
        draw = ImageDraw.Draw(image)
        for tid in targets:
            for points in polys.get(tid, ()):
                if len(points) >= 3:
                    renderer.outline(draw, points, highlight, max(1, round(scale)))
        for points in polys.get(selected, ()):
            if len(points) >= 3:
                renderer.outline(draw, points, highlight, max(2, round(scale * 2.4)))
        self._draw_routes(draw)
        badges = self._draw_badges(draw, state, colors) if state is not None else []

        self._photo = renderer.finish(image)
        # Only the board's own items: effects ("fx") outlive a re-render.
        self.canvas.delete("board")
        self.canvas.create_image(0, self.legend_height, anchor="nw", image=self._photo,
                                 tags="board")
        self.canvas.create_rectangle(
            0, self.legend_height, self.width - 1, self.legend_height + self.height - 1,
            outline=theme["MAP_EDGE"], width=1, tags="board")
        for tid, x, y, text, color in badges:
            self.canvas.create_text(x, y + self.legend_height, text=text, fill=color,
                                    font=(self.family, -self._px(self.BADGE_FONT_PX), "bold"),
                                    tags=("board", f"badge:{tid}"))
        self._draw_legend(state, colors)
        self._draw_frame()
        self.canvas.tag_raise("fx")

    # -- turn frame -----------------------------------------------------------
    def set_frame(self, color: "str | None", width_px: float = 3.0) -> None:
        """Outline the map in `color` -- the player whose turn it is, drawn
        heavier when that is you -- or not at all."""
        if (color, width_px) == (self._frame, self._frame_width):
            return
        self._frame, self._frame_width = color, width_px
        self._draw_frame()

    def _draw_frame(self) -> None:
        self.canvas.delete("turnframe")
        if self._frame is None:
            return
        width = self._px(self._frame_width)
        inset = width / 2.0
        self.canvas.create_rectangle(
            inset, self.legend_height + inset, self.width - inset,
            self.legend_height + self.height - inset, outline=self._frame,
            width=width, tags=("board", "turnframe"))

    def label_xy(self, tid: str) -> "tuple[float, float]":
        """A territory's badge centre in canvas coordinates."""
        x, y = self._renderer.to_px(*self.board.territories[tid].label)
        return x, y + self.legend_height


    def _draw_routes(self, draw) -> None:
        # Mid grey that reads on both oceans.
        draw_routes(self._renderer, draw, self.board.routes, self.theme["WEEK_NUM_FG"],
                    self._px(5), self._px(3.5))

    def _draw_badges(self, draw, state: rules.GameState, colors: dict) -> list:
        """Antialiased badge discs in the bitmap; returns the numerals to put
        on the canvas as (territory, x, y, text, colour) in display pixels."""
        renderer = self._renderer
        scale = renderer.supersample
        radius = self._px(self.BADGE_PX)
        ring = self.theme["CARD_BG"]
        numerals = []
        for tid in self.board.order:
            territory = self.board.territories[tid]
            x, y = renderer.to_px(*territory.label)
            text = str(state.armies[tid])
            color = player_color(self.theme, colors.get(state.owner[tid], -1))
            half = radius + max(0, len(text) - 2) * radius * 0.45
            box = ((x - half) * scale, (y - radius) * scale,
                   (x + half) * scale, (y + radius) * scale)
            draw.rounded_rectangle(box, radius=radius * scale, fill=color,
                                   outline=ring, width=max(1, round(scale * 1.4)))
            numerals.append((tid, x, y, text, text_on(color)))
        return numerals

    def _draw_legend(self, state: "rules.GameState | None", colors: dict) -> None:
        """Continent cells along the top; a continent held by one player
        takes that player's colour."""
        fills = {}
        if state is not None:
            for cid in self.board.continent_order:
                owners = {state.owner[t] for t in self.board.continents[cid].territories}
                if len(owners) == 1:
                    fills[cid] = self.land_fill(colors.get(owners.pop(), -1))
        draw_continent_legend(self.canvas, self.board, self.theme, self.width,
                              self.legend_height, (self.family, -max(7, self._px(9.5))),
                              fills, tags=("board", "legend"))

    # -- hit testing ----------------------------------------------------------
    def territory_at(self, x: float, y: float) -> "str | None":
        """Territory under a canvas point: inside its shape, or else the one
        whose badge is within a finger's width (small islands, Central
        America's isthmus)."""
        y -= self.legend_height
        if y < 0:
            return None
        point = self._renderer.from_px(x, y)
        if point is None:
            return None
        lon, lat = point
        for tid, ring, west, east, south, north in self._hit_rings:
            if west <= lon <= east and south <= lat <= north and _point_in_ring(lon, lat, ring):
                return tid
        slop = self._px(self.HIT_SLOP_PX)
        best, best_d = None, slop * slop
        for tid in self.board.order:
            bx, by = self._renderer.to_px(*self.board.territories[tid].label)
            d = (bx - x) ** 2 + (by - y) ** 2
            if d <= best_d:
                best, best_d = tid, d
        return best

    def _clicked(self, event) -> None:
        tid = self.territory_at(event.x, event.y)
        if tid is not None:
            self._on_click(tid)

    def _moved(self, event) -> None:
        self._set_hover(self.territory_at(event.x, event.y))

    def _set_hover(self, tid: "str | None") -> None:
        if tid == self._hover:
            return
        self._hover = tid
        if self._on_hover is not None:
            self._on_hover(tid)
