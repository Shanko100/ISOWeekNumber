"""Build-time tool: draws WeekNumber's app icon and writes app.ico and app.icns.

The artwork is generated, not traced or downloaded: a violet plate holding a
white calendar page whose highlighted week is the app's own week band. Every
size is rendered at 4x and reduced, and the smallest sizes (16, 24, 32 px)
are drawn as their own simplified shapes rather than shrunk, so they stay
crisp in the taskbar and title bars.

* app.ico (Windows): the plate fills the square, a frame for every window
  and taskbar icon size from 100 % to 300 % scaling, up to 256 px.
* app.icns (macOS): Apple's grid, an 824 px plate on a 1024 px canvas with a
  soft shadow, sizes 16-1024 at 1x and 2x.

Usage, from the repository root: python src/weeknumber/generate_icons.py

With --add-frames it leaves the artwork alone and only adds the frame sizes
app.ico lacks (see add_ico_frames).
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
SUPERSAMPLE = 4

PLATE_TOP = (124, 92, 250)      # #7C5CFA
PLATE_BOTTOM = (67, 56, 202)    # #4338CA
HEADER = (91, 63, 230)          # #5B3FE6
BAND = (124, 92, 250)
DOT = (196, 181, 253)           # #C4B5FD
RING = (237, 233, 254)          # #EDE9FE
WHITE = (255, 255, 255)

#: One frame for every size Windows asks for at the common display scalings
#: (100-300 %): the caption icon is 16-48 px (SM_CXSMICON), the taskbar and
#: Alt+Tab icon 32-96 px (SM_CXICON). With an exact frame nothing is
#: resampled, so the icon stays sharp at 125 %, 150 % and 175 % too.
ICO_SIZES = (16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80, 96, 128, 256)
ICNS_SIZES = (16, 32, 64, 128, 256, 512, 1024)
#: Below this the page is drawn without its day dots and rings.
SIMPLE_BELOW = 48


def _gradient(size: int) -> Image.Image:
    """The plate's diagonal violet gradient."""
    top = Image.new("RGBA", (size, size), PLATE_TOP + (255,))
    bottom = Image.new("RGBA", (size, size), PLATE_BOTTOM + (255,))
    vertical = Image.linear_gradient("L")
    diagonal = ImageChops.add(vertical, vertical.transpose(Image.Transpose.ROTATE_90), scale=2)
    return Image.composite(bottom, top, diagonal.resize((size, size), Image.BILINEAR))


def _plate(size: int, box: "tuple[float, float, float, float]") -> Image.Image:
    """The rounded violet plate with a soft light from above, inside `box`."""
    x0, y0, x1, y1 = box
    side = x1 - x0
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=side * 0.225, fill=255)
    image.paste(_gradient(size), (0, 0), mask)
    glow = Image.new("L", (size, size), 0)
    ImageDraw.Draw(glow).ellipse((x0 - side * 0.2, y0 - side * 0.75, x1 + side * 0.2, y0 + side * 0.42),
                                 fill=70)
    glow = glow.filter(ImageFilter.GaussianBlur(side * 0.06))
    light = Image.new("RGBA", (size, size), WHITE + (0,))
    light.putalpha(Image.composite(glow, Image.new("L", (size, size), 0), mask))
    image.alpha_composite(light)
    return image


def _page(image: Image.Image, box: "tuple[float, float, float, float]", detail: bool) -> None:
    """The calendar page on the plate: header strip, rings, day dots and the
    highlighted week."""
    x0, y0, x1, y1 = box
    side = x1 - x0
    u = side / 1024  # artwork units: the design is laid out on a 1024 grid
    size = image.width
    page = (x0 + 176 * u, y0 + 214 * u, x0 + 848 * u, y0 + 862 * u)
    radius = 90 * u

    if detail:  # a soft shadow under the page
        shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        ImageDraw.Draw(shadow).rounded_rectangle(
            (page[0], page[1] + 26 * u, page[2], page[3] + 26 * u), radius=radius,
            fill=(30, 10, 70, 110))
        image.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(40 * u)))

    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(page, radius=radius, fill=WHITE + (255,))
    header = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(header).rectangle((page[0], page[1], page[2], y0 + 372 * u), fill=HEADER + (255,))
    clip = Image.new("L", (size, size), 0)
    ImageDraw.Draw(clip).rounded_rectangle(page, radius=radius, fill=255)
    image.alpha_composite(Image.composite(header, Image.new("RGBA", (size, size), (0, 0, 0, 0)), clip))

    draw = ImageDraw.Draw(image)
    if detail:
        for cx in (360, 664):  # binder rings
            draw.rounded_rectangle((x0 + (cx - 26) * u, y0 + 150 * u, x0 + (cx + 26) * u, y0 + 270 * u),
                                   radius=26 * u, fill=RING + (255,))
    rows = (450, 548, 646, 744)
    for i, row in enumerate(rows):
        y = y0 + row * u
        if i == 1:  # this week
            draw.rounded_rectangle((x0 + 226 * u, y - 40 * u, x0 + 798 * u, y + 40 * u),
                                   radius=40 * u, fill=BAND + (255,))
        if not detail:
            continue
        for k in range(5):
            x = x0 + (290 + k * 112) * u
            fill = WHITE if i == 1 else DOT
            draw.ellipse((x - 17 * u, y - 17 * u, x + 17 * u, y + 17 * u), fill=fill + (255,))
    if not detail:  # small sizes: two quiet rows instead of dots
        for row in (646, 744):
            y = y0 + row * u
            draw.rounded_rectangle((x0 + 260 * u, y - 22 * u, x0 + 764 * u, y + 22 * u),
                                   radius=22 * u, fill=DOT + (255,))


def render(size: int, inset: float = 0.0, shadow: bool = False) -> Image.Image:
    """The icon at `size` px. `inset` is the margin around the plate as a
    fraction of the size (macOS's grid); `shadow` adds the plate's own."""
    big = size * SUPERSAMPLE
    margin = big * inset
    box = (margin, margin, big - margin, big - margin)
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    if shadow:
        side = box[2] - box[0]
        drop = Image.new("RGBA", (big, big), (0, 0, 0, 0))
        ImageDraw.Draw(drop).rounded_rectangle(
            (box[0], box[1] + side * 0.012, box[2], box[3] + side * 0.012),
            radius=side * 0.225, fill=(0, 0, 0, 90))
        image.alpha_composite(drop.filter(ImageFilter.GaussianBlur(side * 0.02)))
    image.alpha_composite(_plate(big, box))
    _page(image, box, detail=size >= SIMPLE_BELOW)
    return image.resize((size, size), Image.LANCZOS)


#: Apple's grid: an 824 px plate on a 1024 px canvas.
MAC_INSET = 100 / 1024


def write_ico(path: Path) -> None:
    frames = [render(size) for size in ICO_SIZES]
    frames[-1].save(path, format="ICO", sizes=[(s, s) for s in ICO_SIZES],
                    append_images=frames[:-1])


def write_icns(path: Path) -> None:
    frames = [render(size, MAC_INSET, shadow=size >= SIMPLE_BELOW) for size in ICNS_SIZES]
    frames[-1].save(path, format="ICNS", append_images=frames[:-1])


def add_ico_frames(path: Path, sizes: "tuple[int, ...]" = ICO_SIZES) -> "list[int]":
    """Give an existing .ico the frames of `sizes` it lacks, resampled from
    its largest frame; the frames it has are kept byte for byte. Returns the
    sizes added. This is how the shipped app.ico got its in-between sizes
    without its artwork changing."""
    import io
    import struct

    data = path.read_bytes()
    reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    if reserved != 0 or kind != 1 or not 0 < count <= 64:
        raise ValueError(f"{path.name} is not an icon file")
    frames: dict[int, bytes] = {}
    for index in range(count):
        width, height, _colors, _res, _planes, _bpp, size, offset = struct.unpack_from(
            "<BBBBHHII", data, 6 + 16 * index)
        width, height = width or 256, height or 256
        if width == height:
            frames[width] = data[offset:offset + size]
    largest = max(frames)
    with Image.open(io.BytesIO(frames[largest])) as source:
        source = source.convert("RGBA")
    added = []
    for size in sizes:
        if size in frames or size > largest:
            continue
        buffer = io.BytesIO()
        source.resize((size, size), Image.LANCZOS).save(buffer, format="PNG", optimize=True)
        frames[size] = buffer.getvalue()
        added.append(size)
    order = sorted(frames)
    header = struct.pack("<HHH", 0, 1, len(order))
    offset = 6 + 16 * len(order)
    entries, blobs = [], []
    for size in order:
        blob = frames[size]
        entries.append(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32,
                                   len(blob), offset))
        blobs.append(blob)
        offset += len(blob)
    path.write_bytes(header + b"".join(entries) + b"".join(blobs))
    return added


def main() -> None:
    import sys
    if "--add-frames" in sys.argv[1:]:
        added = add_ico_frames(HERE / "app.ico")
        print(f"Added {added or 'no'} px frames to {HERE / 'app.ico'}")
        return
    write_ico(HERE / "app.ico")
    write_icns(HERE / "app.icns")
    print(f"Wrote {HERE / 'app.ico'} and {HERE / 'app.icns'}")


if __name__ == "__main__":
    main()
