"""World-map rendering shared by the time-zone picker and the Timezone Dominator board.

Both maps are the same drawing: the plate-carree projection from
weeknumber_timezone, an ocean washed with the time-zone band tints, land
filled region by region, hairline borders on top, all rendered with Pillow
at an oversampled size and resampled down. Tk's canvas cannot antialias,
which is what made coastlines and borders look ragged and turned small
islands into hard specks; Pillow is already a dependency for the tray icon,
so the map is rendered oversized and resampled down, exactly as IconRenderer
does. Keeping one renderer is what makes the two maps look identical by
construction.

The renderer owns geometry and pixels only; each dialog decides what to fill
with which colour and composes its own overlays (selection, army badges) on
a copy of the cached base image.
"""

from __future__ import annotations

from collections.abc import Iterable

from PIL import Image, ImageDraw, ImageTk

from weeknumber_timezone import ASPECT, WORLD_BORDERS, ZONE_BANDS, project, unproject
from weeknumber_widgets import blend


def fit_map_size(preferred: float, minimum: float, available_width: float,
                 available_height: float) -> tuple[int, int]:
    """The largest map that fits the space left after the dialog's chrome,
    never below `minimum` wide, keeping the projection's aspect ratio."""
    width = min(float(preferred), float(available_width),
                max(1.0, available_height) * ASPECT)
    width = max(float(minimum), width)
    return int(round(width)), max(1, int(round(width / ASPECT)))


class WorldMapRenderer:
    #: Oversampling factor. 2x antialiases coastlines and borders as well as
    #: 3x did once the hairline below is one bitmap pixel, at under half the
    #: memory: the board's bitmap was the game's largest allocation (10 MB at
    #: 3x for a 980 px map, plus a working copy per render).
    SUPERSAMPLE = 2
    #: How far a band's sea tint sits from its land colour, toward the open
    #: ocean. Keeps the two readable as one zone while land stays dominant.
    SEA_MIX = 0.74

    def __init__(self, theme: dict[str, str]) -> None:
        self.theme = theme
        self.width = 10
        self.height = 10

    def resize(self, width: int, height: int) -> None:
        self.width = max(1, int(width))
        self.height = max(1, int(height))

    @property
    def supersample(self) -> int:
        return self.SUPERSAMPLE

    # -- coordinates --------------------------------------------------------
    def to_px(self, lon: float, lat: float) -> tuple[float, float]:
        """Degrees to display pixels, origin at the map body's top-left."""
        nx, ny = project(lon, lat)
        half_w = self.width / 2.0
        half_h = self.height / 2.0
        return half_w + nx * half_w, half_h - ny * half_h

    def from_px(self, x: float, y: float) -> "tuple[float, float] | None":
        """Display pixels back to degrees; None outside the map body."""
        half_w = self.width / 2.0
        half_h = self.height / 2.0
        return unproject((x - half_w) / half_w, (half_h - y) / half_h)

    def hi_poly(self, ring: list[float]) -> list:
        """A flat [lon, lat, ...] ring as Pillow points inside the oversampled
        bitmap, whose origin is the map body."""
        scale = self.supersample
        half_w = self.width * scale / 2.0
        half_h = self.height * scale / 2.0
        points = []
        for i in range(0, len(ring), 2):
            nx, ny = project(ring[i], ring[i + 1])
            points.append((half_w + nx * half_w, half_h - ny * half_h))
        return points

    @staticmethod
    def outline(draw, points, color: str, width: int) -> None:
        """Trace a closed ring. Done with a line rather than polygon(outline=)
        because that keyword's width argument is a recent Pillow addition and
        the build only guarantees 10.1."""
        draw.line(points + [points[0]], fill=color, width=width, joint="curve")

    # -- painting -----------------------------------------------------------
    def sea_color(self, tint: int, mix: "float | None" = None) -> str:
        return blend(self.theme[f"MAP_BAND_{tint}"], self.theme["MAP_OCEAN"],
                     self.SEA_MIX if mix is None else mix)

    def new_image(self) -> "tuple[Image.Image, ImageDraw.ImageDraw]":
        scale = self.supersample
        size = (max(1, self.width * scale), max(1, self.height * scale))
        image = Image.new("RGB", size, self.theme["MAP_OCEAN"])
        return image, ImageDraw.Draw(image)

    def paint_band_seas(self, draw, mix: "float | None" = None) -> None:
        """Every band's diluted tint over open water."""
        for band in ZONE_BANDS:
            fill = self.sea_color(band.tint, mix)
            self.paint_rings(draw, band.sea, fill)

    def paint_band_land(self, draw, fill: "str | None" = None) -> None:
        """Every band's land, in its band colour or one shared `fill`."""
        for band in ZONE_BANDS:
            self.paint_rings(draw, band.land,
                             fill or self.theme[f"MAP_BAND_{band.tint}"])

    def paint_rings(self, draw, rings: Iterable[list[float]], fill: str,
                    outline: "str | None" = None, width: int = 1) -> None:
        self.paint_polys(draw, (self.hi_poly(ring) for ring in rings), fill,
                         outline, width)

    def paint_polys(self, draw, polys: Iterable[list], fill: str,
                    outline: "str | None" = None, width: int = 1) -> None:
        """paint_rings for rings already projected with hi_poly."""
        for points in polys:
            if len(points) >= 3:
                draw.polygon(points, fill=fill)
                if outline is not None:
                    self.outline(draw, points, outline, width)

    def paint_borders(self, draw, color: "str | None" = None) -> None:
        border = color or self.theme["MAP_BORDER_LINE"]
        hairline = self.hairline
        for line in WORLD_BORDERS:
            points = self.hi_poly(line)
            if len(points) >= 2:
                draw.line(points, fill=border, width=hairline, joint="curve")

    @property
    def hairline(self) -> int:
        # About half a display pixel: crisp, and as light as the old 3x line.
        return max(1, round(self.supersample * 0.6))

    def downsample(self, image: "Image.Image") -> "Image.Image":
        """The oversampled bitmap at display size. new_image() is an exact
        multiple of it, so a box reduction does the job: it averages each
        supersample block like LANCZOS would, an order of magnitude faster
        (about 1 ms against 20 for the default map)."""
        scale = self.supersample
        if image.size == (self.width * scale, self.height * scale):
            return image.reduce(scale)
        return image.resize((self.width, self.height), Image.LANCZOS)

    def finish(self, image: "Image.Image") -> "ImageTk.PhotoImage":
        """Resample the oversampled bitmap to display size for Tk."""
        return ImageTk.PhotoImage(self.downsample(image))
