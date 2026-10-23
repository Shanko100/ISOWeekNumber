"""Generate WeekNumber's bundled Timezone Dominator board, ``tzd_data.json``.

This is a build-time tool only. The installed application reads the generated
JSON and never imports shapely or accesses the network.

Inputs are Natural Earth 5.1.2 (public domain) GeoJSON layers, downloaded
once by the maintainer:

    ne_50m_admin_0_countries.geojson
    ne_50m_admin_1_states_provinces.geojson

from https://github.com/nvkelso/natural-earth-vector/tree/v5.1.2/geojson.
Countries are assigned to the 42 classic territories; the United States,
Canada, Russia, China and Australia are split by their first-level
subdivisions. Where real geography and the classic board disagree the board
wins: Kazakhstan joins Ural and Manchuria and Korea join Mongolia, so those
territories touch the neighbours the rules say they touch.
Overseas parts of a country (French Guiana, the Canaries, Borneo's Malaysian
states...) are routed by location instead of by owner. Each territory is
dissolved, simplified and clipped to the time-zone map's latitude window, so
the board lines up with the map it is drawn over.

Usage:
    python generate_tzd_data.py <directory with the two GeoJSON files>
"""

from __future__ import annotations

import datetime as dt
import json
import os
import sys
import tempfile
from pathlib import Path

from shapely.geometry import LineString, Polygon, box, shape
from shapely.geometry.base import BaseGeometry
from shapely.ops import nearest_points, polylabel, unary_union

BASE_DIR = Path(__file__).resolve().parent
OUTPUT_PATH = BASE_DIR / "tzd_data.json"
FORMAT = "weeknumber-tzd-1"
NATURAL_EARTH_VERSION = "5.1.2"
ADMIN0 = "ne_50m_admin_0_countries.geojson"
ADMIN1 = "ne_50m_admin_1_states_provinces.geojson"
MAX_INPUT_BYTES = 32 * 1024 * 1024
#: Must match weeknumber_timezone.LAT_MIN / LAT_MAX.
LAT_MIN, LAT_MAX = -58.0, 84.0
SIMPLIFY_DEG = 0.08
#: Island parts smaller than this (square degrees) are dropped, except a
#: territory's largest part. Keeps specks out of the point budget.
MIN_PART_AREA = 0.04
#: Coasts closer than this are one land border, not a sea crossing.
TOUCH_DEG = 0.25
#: A route crossing more of third-party land than this share is not drawn.
MAX_ROUTE_OVER_LAND = 0.15

CONTINENTS = [
    ("north_america", "North America", 5),
    ("south_america", "South America", 2),
    ("europe", "Europe", 5),
    ("africa", "Africa", 3),
    ("asia", "Asia", 7),
    ("australia", "Australia", 2),
]

#: (id, display name, continent). Order is the board's reading order.
TERRITORIES = [
    ("alaska", "Alaska", "north_america"),
    ("northwest_territory", "Northwest Territory", "north_america"),
    ("greenland", "Greenland", "north_america"),
    ("alberta", "Alberta", "north_america"),
    ("ontario", "Ontario", "north_america"),
    ("quebec", "Quebec", "north_america"),
    ("western_us", "Western United States", "north_america"),
    ("eastern_us", "Eastern United States", "north_america"),
    ("central_america", "Central America", "north_america"),
    ("venezuela", "Venezuela", "south_america"),
    ("peru", "Peru", "south_america"),
    ("brazil", "Brazil", "south_america"),
    ("argentina", "Argentina", "south_america"),
    ("iceland", "Iceland", "europe"),
    ("great_britain", "Great Britain", "europe"),
    ("scandinavia", "Scandinavia", "europe"),
    ("northern_europe", "Northern Europe", "europe"),
    ("western_europe", "Western Europe", "europe"),
    ("southern_europe", "Southern Europe", "europe"),
    ("ukraine", "Ukraine", "europe"),
    ("north_africa", "North Africa", "africa"),
    ("egypt", "Egypt", "africa"),
    ("east_africa", "East Africa", "africa"),
    ("congo", "Congo", "africa"),
    ("south_africa", "South Africa", "africa"),
    ("madagascar", "Madagascar", "africa"),
    ("ural", "Ural", "asia"),
    ("siberia", "Siberia", "asia"),
    ("yakutsk", "Yakutsk", "asia"),
    ("kamchatka", "Kamchatka", "asia"),
    ("irkutsk", "Irkutsk", "asia"),
    ("mongolia", "Mongolia", "asia"),
    ("japan", "Japan", "asia"),
    ("afghanistan", "Afghanistan", "asia"),
    ("middle_east", "Middle East", "asia"),
    ("india", "India", "asia"),
    ("china", "China", "asia"),
    ("siam", "Siam", "asia"),
    ("indonesia", "Indonesia", "australia"),
    ("new_guinea", "New Guinea", "australia"),
    ("western_australia", "Western Australia", "australia"),
    ("eastern_australia", "Eastern Australia", "australia"),
]

#: The classic board's adjacency, each pair once.
ADJACENCY = [
    ("alaska", "northwest_territory"), ("alaska", "alberta"), ("alaska", "kamchatka"),
    ("northwest_territory", "alberta"), ("northwest_territory", "ontario"),
    ("northwest_territory", "greenland"),
    ("greenland", "ontario"), ("greenland", "quebec"), ("greenland", "iceland"),
    ("alberta", "ontario"), ("alberta", "western_us"),
    ("ontario", "western_us"), ("ontario", "eastern_us"), ("ontario", "quebec"),
    ("quebec", "eastern_us"),
    ("western_us", "eastern_us"), ("western_us", "central_america"),
    ("eastern_us", "central_america"),
    ("central_america", "venezuela"),
    ("venezuela", "peru"), ("venezuela", "brazil"),
    ("peru", "brazil"), ("peru", "argentina"),
    ("brazil", "argentina"), ("brazil", "north_africa"),
    ("iceland", "great_britain"), ("iceland", "scandinavia"),
    ("great_britain", "scandinavia"), ("great_britain", "northern_europe"),
    ("great_britain", "western_europe"),
    ("scandinavia", "northern_europe"), ("scandinavia", "ukraine"),
    ("northern_europe", "ukraine"), ("northern_europe", "southern_europe"),
    ("northern_europe", "western_europe"),
    ("western_europe", "southern_europe"), ("western_europe", "north_africa"),
    ("southern_europe", "ukraine"), ("southern_europe", "middle_east"),
    ("southern_europe", "egypt"), ("southern_europe", "north_africa"),
    ("ukraine", "middle_east"), ("ukraine", "afghanistan"), ("ukraine", "ural"),
    ("north_africa", "egypt"), ("north_africa", "east_africa"), ("north_africa", "congo"),
    ("egypt", "middle_east"), ("egypt", "east_africa"),
    ("east_africa", "congo"), ("east_africa", "south_africa"),
    ("east_africa", "madagascar"), ("east_africa", "middle_east"),
    ("congo", "south_africa"),
    ("south_africa", "madagascar"),
    ("ural", "siberia"), ("ural", "china"), ("ural", "afghanistan"),
    ("siberia", "yakutsk"), ("siberia", "irkutsk"), ("siberia", "mongolia"),
    ("siberia", "china"),
    ("yakutsk", "irkutsk"), ("yakutsk", "kamchatka"),
    ("kamchatka", "irkutsk"), ("kamchatka", "mongolia"), ("kamchatka", "japan"),
    ("irkutsk", "mongolia"),
    ("mongolia", "japan"), ("mongolia", "china"),
    ("afghanistan", "china"), ("afghanistan", "india"), ("afghanistan", "middle_east"),
    ("middle_east", "india"),
    ("india", "china"), ("india", "siam"),
    ("china", "siam"),
    ("siam", "indonesia"),
    ("indonesia", "new_guinea"), ("indonesia", "western_australia"),
    ("new_guinea", "western_australia"), ("new_guinea", "eastern_australia"),
    ("western_australia", "eastern_australia"),
]

#: Natural Earth ADM0_A3 -> territory. Countries absent here are not on the
#: board (Antarctica, remote Pacific states...) and stay neutral land.
COUNTRY_TERRITORY = {
    "GRL": "greenland",
    **dict.fromkeys(["MEX", "GTM", "BLZ", "HND", "SLV", "NIC", "CRI", "PAN", "CUB",
                     "JAM", "HTI", "DOM", "BHS", "PRI", "BRB", "LCA", "VCT", "GRD",
                     "ATG", "KNA", "DMA", "CYM", "TCA", "VGB", "VIR", "ABW", "CUW",
                     "BLM", "MAF", "SXM", "AIA", "MSR"], "central_america"),
    "BMU": "eastern_us",
    **dict.fromkeys(["VEN", "COL", "GUY", "SUR", "TTO"], "venezuela"),
    **dict.fromkeys(["PER", "ECU", "BOL"], "peru"),
    "BRA": "brazil",
    **dict.fromkeys(["ARG", "CHL", "URY", "PRY", "FLK"], "argentina"),
    **dict.fromkeys(["ISL", "FRO"], "iceland"),
    **dict.fromkeys(["GBR", "IRL", "IMN", "GGY", "JEY"], "great_britain"),
    **dict.fromkeys(["NOR", "SWE", "FIN", "ALD"], "scandinavia"),
    **dict.fromkeys(["DEU", "NLD", "BEL", "LUX", "DNK", "POL", "CZE", "SVK", "AUT",
                     "CHE", "LIE"], "northern_europe"),
    **dict.fromkeys(["FRA", "ESP", "PRT", "AND", "MCO"], "western_europe"),
    **dict.fromkeys(["ITA", "SMR", "VAT", "MLT", "SVN", "HRV", "BIH", "SRB", "MNE",
                     "KOS", "ALB", "MKD", "GRC", "BGR", "ROU", "HUN"], "southern_europe"),
    **dict.fromkeys(["UKR", "BLR", "MDA", "EST", "LVA", "LTU"], "ukraine"),
    **dict.fromkeys(["MAR", "SAH", "DZA", "TUN", "MRT", "MLI", "NER", "TCD", "SEN",
                     "GMB", "GNB", "GIN", "SLE", "LBR", "CIV", "BFA", "GHA", "TGO",
                     "BEN", "NGA", "CPV"], "north_africa"),
    **dict.fromkeys(["EGY", "LBY"], "egypt"),
    **dict.fromkeys(["SDN", "SDS", "ETH", "ERI", "DJI", "SOM", "SOL", "KEN", "UGA",
                     "TZA", "RWA", "BDI"], "east_africa"),
    **dict.fromkeys(["COD", "COG", "CAF", "CMR", "GAB", "GNQ", "AGO", "STP"], "congo"),
    **dict.fromkeys(["ZAF", "NAM", "BWA", "ZWE", "ZMB", "MOZ", "MWI", "LSO", "SWZ"],
                    "south_africa"),
    **dict.fromkeys(["MDG", "COM", "MUS", "SYC"], "madagascar"),
    **dict.fromkeys(["MNG", "PRK", "KOR"], "mongolia"),
    "JPN": "japan",
    "KAZ": "ural",
    **dict.fromkeys(["AFG", "TKM", "UZB", "TJK", "KGZ"], "afghanistan"),
    **dict.fromkeys(["TUR", "CYP", "CYN", "SYR", "LBN", "ISR", "PSX", "JOR", "IRQ",
                     "IRN", "SAU", "KWT", "BHR", "QAT", "ARE", "OMN", "YEM", "GEO",
                     "ARM", "AZE"], "middle_east"),
    **dict.fromkeys(["IND", "PAK", "NPL", "BTN", "BGD", "LKA", "MDV", "KAS"], "india"),
    **dict.fromkeys(["TWN", "HKG", "MAC"], "china"),
    **dict.fromkeys(["THA", "MMR", "LAO", "KHM", "VNM", "MYS", "SGP"], "siam"),
    **dict.fromkeys(["IDN", "BRN", "TLS", "PHL"], "indonesia"),
    **dict.fromkeys(["PNG", "SLB"], "new_guinea"),
    **dict.fromkeys(["NZL", "NCL", "VUT", "FJI"], "eastern_australia"),
}

#: Countries taken from their subdivisions instead: (ADM0_A3, name) -> territory.
SPLIT_COUNTRIES = {"USA", "CAN", "RUS", "CHN", "AUS"}
_MANCHURIA = ["Inner Mongol", "Heilongjiang", "Jilin", "Liaoning"]
_US_WEST = ["Washington", "Oregon", "California", "Nevada", "Idaho", "Montana",
            "Wyoming", "Utah", "Arizona", "New Mexico", "Colorado", "North Dakota",
            "South Dakota", "Nebraska", "Kansas", "Oklahoma", "Texas", "Hawaii"]
_RUS = {
    "ural": ["Sverdlovsk", "Chelyabinsk", "Kurgan", "Tyumen'", "Khanty-Mansiy",
             "Yamal-Nenets", "Perm'", "Bashkortostan", "Orenburg"],
    "siberia": ["Omsk", "Tomsk", "Novosibirsk", "Altay", "Gorno-Altay", "Kemerovo",
                "Krasnoyarsk", "Khakass", "Tuva"],
    "irkutsk": ["Irkutsk", "Buryat", "Chita", "Amur"],
    "yakutsk": ["Sakha (Yakutia)"],
    "kamchatka": ["Kamchatka", "Chukchi Autonomous Okrug", "Maga Buryatdan",
                  "Khabarovsk", "Primor'ye", "Yevrey", "Sakhalin"],
}


def subdivision_territory(country: str, name: str) -> "str | None":
    if country == "USA":
        if name == "Alaska":
            return "alaska"
        return "western_us" if name in _US_WEST else "eastern_us"
    if country == "CAN":
        if name in ("Yukon", "Northwest Territories", "Nunavut"):
            return "northwest_territory"
        if name in ("British Columbia", "Alberta", "Saskatchewan"):
            return "alberta"
        if name in ("Manitoba", "Ontario"):
            return "ontario"
        return "quebec"
    if country == "RUS":
        for territory, names in _RUS.items():
            if name in names:
                return territory
        return "ukraine"  # European Russia
    if country == "CHN":
        return "mongolia" if name in _MANCHURIA else "china"
    if country == "AUS":
        if name in ("Western Australia", "Northern Territory", "South Australia"):
            return "western_australia"
        return "eastern_australia"
    return None


#: Where a country's own parts are expected (lon0, lat0, lon1, lat1). A part
#: whose centre falls outside is overseas and routed by OVERSEAS instead.
HOME = {
    "FRA": (-6, 41, 10, 52), "ESP": (-10, 35, 5, 44.5), "PRT": (-32, 32, -6, 43),
    "NLD": (3, 50, 8, 54), "GBR": (-11, 49, 2.5, 61.5), "IDN": (94, -11.5, 131, 6.5),
    "MYS": (99, 0.5, 105, 7.5), "ECU": (-92.5, -6, -75, 2), "CHL": (-76, -56.5, -66, -17),
    "NOR": (4, 57, 32, 81.5), "DNK": (8, 54, 16, 58), "NZL": (165, -53, 179.99, -34),
}
#: Destination boxes for overseas parts, checked in order.
OVERSEAS = [
    ((-90, 8, -59, 28), "central_america"),       # Caribbean
    ((-55, 1.5, -51, 6.5), "venezuela"),          # French Guiana
    ((-19, 27, -13, 30), "north_africa"),         # Canary Islands
    ((40, -26, 60, -11), "madagascar"),           # Reunion, Mayotte
    ((160, -23, 172, -19), "eastern_australia"),  # New Caledonia
    ((130.8, -11, 141.2, 0.5), "new_guinea"),     # Western New Guinea
    ((108, -1, 120, 8), "indonesia"),             # Malaysian Borneo
]

#: Hand-placed army-badge anchors where the pole of inaccessibility of the
#: largest part reads poorly (long thin shapes, archipelagos).
LABEL_OVERRIDES = {
    "central_america": (-101.0, 21.5),
    "great_britain": (-2.0, 53.5),
    "japan": (138.5, 36.5),
    "indonesia": (114.0, -1.0),
    "kamchatka": (160.0, 64.0),
    "northwest_territory": (-115.0, 66.0),
    "scandinavia": (16.0, 63.0),
    "southern_europe": (21.0, 44.0),
    "middle_east": (44.0, 27.0),
    "argentina": (-65.0, -35.0),
    "eastern_us": (-86.0, 37.0),
}


#: Hand-drawn sea routes (lon, lat, lon, lat) where the nearest coastal points
#: land on a small island instead of the coast a reader expects.
LINK_OVERRIDES = {
    ("brazil", "north_africa"): (-35.2, -5.5, -17.2, 14.7),
    ("east_africa", "madagascar"): (39.6, -7.6, 49.2, -12.3),
    ("south_africa", "madagascar"): (35.4, -23.9, 43.6, -23.4),
    ("iceland", "scandinavia"): (-13.6, 65.1, 5.0, 61.9),
    ("iceland", "great_britain"): (-14.6, 64.2, -5.0, 58.6),
    ("mongolia", "japan"): (129.0, 35.1, 130.9, 33.9),
}


def _load(path: Path) -> list:
    payload = path.read_bytes()
    if len(payload) > MAX_INPUT_BYTES:
        raise RuntimeError(f"{path.name} exceeds the input size limit")
    document = json.loads(payload.decode("utf-8"))
    features = document.get("features")
    if not isinstance(features, list):
        raise RuntimeError(f"{path.name} has no feature list")
    return features


def _parts(geometry: BaseGeometry) -> "list[Polygon]":
    if geometry.is_empty:
        return []
    if geometry.geom_type == "Polygon":
        return [geometry]
    return [g for g in getattr(geometry, "geoms", []) if g.geom_type == "Polygon"]


def _inside(point, bounds) -> bool:
    lon0, lat0, lon1, lat1 = bounds
    return lon0 <= point.x <= lon1 and lat0 <= point.y <= lat1


def _route(country: str, home_territory: str, part: Polygon) -> "str | None":
    home = HOME.get(country)
    centre = part.representative_point()
    if home is None or _inside(centre, home):
        return home_territory
    for bounds, territory in OVERSEAS:
        if _inside(centre, bounds):
            return territory
    return None  # remote dependency: not on the board


def collect(source_dir: Path) -> "dict[str, list[Polygon]]":
    pieces: dict[str, list[Polygon]] = {tid: [] for tid, _n, _c in TERRITORIES}
    for feature in _load(source_dir / ADMIN0):
        props = feature["properties"]
        code = props["ADM0_A3"]
        if code in SPLIT_COUNTRIES:
            continue
        territory = COUNTRY_TERRITORY.get(code)
        if territory is None:
            continue
        for part in _parts(shape(feature["geometry"]).buffer(0)):
            routed = _route(code, territory, part)
            if routed is not None:
                pieces[routed].append(part)
    for feature in _load(source_dir / ADMIN1):
        props = feature["properties"]
        code = props["adm0_a3"]
        if code not in SPLIT_COUNTRIES:
            continue
        territory = subdivision_territory(code, props["name"])
        if territory is None:
            continue
        pieces[territory].extend(_parts(shape(feature["geometry"]).buffer(0)))
    return pieces


def build_geometry(parts: "list[Polygon]") -> BaseGeometry:
    window = box(-180.0, LAT_MIN, 180.0, LAT_MAX)
    merged = unary_union(parts).buffer(0).intersection(window)
    simplified = merged.simplify(SIMPLIFY_DEG, preserve_topology=True)
    polygons = sorted(_parts(simplified), key=lambda p: p.area, reverse=True)
    kept = [p for i, p in enumerate(polygons) if i == 0 or p.area >= MIN_PART_AREA]
    return unary_union(kept)


def _ring(polygon: Polygon) -> "list[float]":
    coords = list(polygon.exterior.coords)[:-1]  # drop the closing duplicate
    flat: list[float] = []
    for lon, lat in coords:
        flat += [round(lon, 2), round(lat, 2)]
    return flat


def build_links(geometries: "dict[str, BaseGeometry]") -> list:
    """Every adjacency, plus the dashed route to draw for a sea crossing.

    A pair that does not share a border but whose shortest connection runs
    over another territory's land is an abstract board link (Ural -- China
    style); it gets no line, and the game highlights valid targets instead.
    """
    land = unary_union(list(geometries.values()))
    links = []
    for a, b in ADJACENCY:
        ga, gb = geometries[a], geometries[b]
        link: dict = {"a": a, "b": b}
        override = LINK_OVERRIDES.get((a, b))
        if override is not None:
            link["line"] = list(override)
        elif ga.distance(gb) > TOUCH_DEG:
            pa, pb = nearest_points(ga, gb)
            route = LineString([pa, pb])
            others = land.difference(ga.buffer(0.05)).difference(gb.buffer(0.05))
            # Mostly over water: a real crossing worth drawing.
            if route.intersection(others).length <= MAX_ROUTE_OVER_LAND * route.length:
                link["line"] = [round(pa.x, 2), round(pa.y, 2),
                                round(pb.x, 2), round(pb.y, 2)]
        links.append(link)
    return links


def build(source_dir: Path) -> dict:
    pieces = collect(source_dir)
    geometries: dict[str, BaseGeometry] = {}
    territories = []
    total_points = 0
    for tid, name, continent in TERRITORIES:
        if not pieces[tid]:
            raise RuntimeError(f"No geometry was assigned to {tid}")
        geometry = build_geometry(pieces[tid])
        geometries[tid] = geometry
        polygons = sorted(_parts(geometry), key=lambda p: p.area, reverse=True)
        rings = [_ring(p) for p in polygons if len(p.exterior.coords) >= 4]
        total_points += sum(len(r) // 2 for r in rings)
        if tid in LABEL_OVERRIDES:
            label = LABEL_OVERRIDES[tid]
        else:
            anchor = polylabel(polygons[0], tolerance=0.05)
            label = (anchor.x, anchor.y)
        territories.append({
            "id": tid, "name": name, "continent": continent,
            "label": [round(label[0], 2), round(label[1], 2)],
            "rings": rings,
        })
    links = build_links(geometries)
    print(f"{len(territories)} territories, {total_points} points, "
          f"{sum(1 for link in links if 'line' in link)} sea routes")
    return {
        "format": FORMAT,
        "generated": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat(),
        "sources": [
            f"Territory shapes: Natural Earth {NATURAL_EARTH_VERSION} "
            "(ne_50m_admin_0_countries, ne_50m_admin_1_states_provinces), public domain, "
            "dissolved into the game's territories, simplified and clipped",
        ],
        "continents": [{"id": cid, "name": name, "bonus": bonus}
                       for cid, name, bonus in CONTINENTS],
        "territories": territories,
        "links": links,
    }


def write(document: dict) -> None:
    text = json.dumps(document, ensure_ascii=False, separators=(",", ":"))
    fd, tmp_name = tempfile.mkstemp(prefix="tzd-", suffix=".json.tmp", dir=str(BASE_DIR))
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text + "\n")
        os.chmod(tmp, 0o644)  # mkstemp creates 0600; the file ships with the app
        os.replace(tmp, OUTPUT_PATH)
    finally:
        if tmp.exists():
            tmp.unlink()


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    write(build(Path(sys.argv[1])))
    print(f"Wrote {OUTPUT_PATH}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
