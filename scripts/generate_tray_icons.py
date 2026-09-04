#!/usr/bin/env python3
"""Genera le icone RGBA grezze della tray da src-tauri/icons/tray/tray-idle.png.

tauri::image::Image::new (vedi src-tauri/src/tray.rs) vuole pixel RGBA già
decodificati, non PNG — questo script produce sia i .rgba consumati da
include_bytes! sia dei .png accanto, solo per un'ispezione visiva comoda
(non letti dal codice Rust).

L'icona principale è "a maschera" (un solo colore, chiaro o scuro) e si
adatta allo sfondo della tray in base a `tray.rs::detect_tray_base` — niente
più tinte teal/arancio/rosso sull'icona intera. Priorità degli stati:

- problema: icona col logo + badge rosso (punto esclamativo) in basso a
  destra.
- aggiornamento: icona col logo + badge giallo (freccia in su).
- in corso: **l'icona intera** diventa le due classiche frecce circolari di
  sync/aggiornamento (non un badge) — un badge con due archi e relative
  punte non è leggibile alla dimensione reale della tray (~16-24px), stesso
  motivo per cui client come Nextcloud Desktop o Insync sostituiscono
  l'icona intera durante la sincronizzazione invece di usare un overlay
  minuscolo.
- nessuno: solo il logo.

Uso: python3 scripts/generate_tray_icons.py
Richiede Pillow (pip install Pillow).
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SOURCE_PNG = ROOT / "src-tauri" / "icons" / "tray" / "tray-idle.png"
OUT_DIR = ROOT / "src-tauri" / "icons" / "tray"

WORK_SIZE = 512  # dimensione del PNG sorgente, lavora qui per bordi puliti
FINAL_SIZE = 64  # dimensione consumata da Image::new in tray.rs

# Icona "a maschera": bianca per sedere su una tray scura, quasi nera per
# sederne su una chiara — nessuna sfumatura intermedia, stesso principio
# delle "template image" di macOS.
ICON_LIGHT = (255, 255, 255)
ICON_DARK = (20, 20, 20)

BADGE_YELLOW = (240, 196, 25)  # "aggiornamento disponibile"
BADGE_RED = (207, 34, 46)  # "problema" — coincide con --error del frontend

# Diametro badge relativo a WORK_SIZE: abbastanza grande da restare
# leggibile anche schiacciato alla dimensione reale della tray, ma non tanto
# da far sforare l'anello di contrasto oltre il bordo del canvas dato
# BADGE_CENTER (un diametro di 0.52 lo sfora).
BADGE_DIAMETER = round(WORK_SIZE * 0.46)
BADGE_RADIUS = BADGE_DIAMETER // 2
BADGE_CENTER = (round(WORK_SIZE * 0.74), round(WORK_SIZE * 0.74))
RING_WIDTH = round(WORK_SIZE * 0.012)


def recolor(base: Image.Image, rgb: tuple[int, int, int]) -> Image.Image:
    """Sostituisce l'RGB di ogni pixel con `rgb`, mantiene l'alpha originale
    (il logo è già a tinta piatta — nessuna sfumatura da preservare)."""
    alpha = base.split()[3]
    solid = Image.new("RGBA", base.size, rgb + (255,))
    solid.putalpha(alpha)
    return solid


def draw_badge_circle(canvas: Image.Image, fill: tuple[int, int, int], ring: tuple[int, int, int]) -> ImageDraw.ImageDraw:
    """Cerchio pieno con un anello dello stesso colore dell'icona base
    (chiaro o scuro a seconda del tema della tray, non un bianco fisso —
    così il badge resta "attaccato" visivamente all'icona invece di
    introdurre un terzo colore), ritorna il ImageDraw pronto per il glifo
    sopra."""
    draw = ImageDraw.Draw(canvas)
    cx, cy = BADGE_CENTER
    r = BADGE_RADIUS
    draw.ellipse([cx - r - RING_WIDTH, cy - r - RING_WIDTH, cx + r + RING_WIDTH, cy + r + RING_WIDTH], fill=ring + (255,))
    draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill + (255,))
    return draw


def draw_circular_arrows(draw: ImageDraw.ImageDraw, center: tuple[float, float], radius: float, width: float, color: tuple[int, int, int]) -> None:
    """Le classiche due frecce circolari di un'icona "sincronizza": due
    archi opposti, ciascuno con una punta a triangolo tangente al cerchio
    nella direzione di percorrenza dell'arco."""
    cx, cy = center
    head_len = width * 2.4
    head_w = width * 1.5
    fill = color + (255,)

    for start_deg, end_deg in ((15, 155), (195, 335)):
        bbox = [cx - radius, cy - radius, cx + radius, cy + radius]
        draw.arc(bbox, start=start_deg, end=end_deg, fill=fill, width=round(width))

        theta = math.radians(end_deg)
        tip = (cx + radius * math.cos(theta), cy + radius * math.sin(theta))
        # tangente nella direzione di theta crescente (verso di percorrenza
        # dell'arco appena disegnato) e normale (= direzione radiale)
        tangent = (-math.sin(theta), math.cos(theta))
        normal = (math.cos(theta), math.sin(theta))
        base = (tip[0] - tangent[0] * head_len, tip[1] - tangent[1] * head_len)
        p1 = (base[0] + normal[0] * head_w, base[1] + normal[1] * head_w)
        p2 = (base[0] - normal[0] * head_w, base[1] - normal[1] * head_w)
        draw.polygon([tip, p1, p2], fill=fill)


def draw_update_glyph(draw: ImageDraw.ImageDraw) -> None:
    """Freccia verso l'alto (triangolo + gambo), scura per restare leggibile
    sul giallo (un glifo bianco si confonderebbe troppo)."""
    cx, cy = BADGE_CENTER
    s = BADGE_RADIUS * 0.66
    dark = (40, 34, 0, 255)
    tip_y = cy - s
    base_y = cy + s * 0.15
    draw.polygon([(cx - s * 0.75, cy), (cx + s * 0.75, cy), (cx, tip_y)], fill=dark)
    stem_w = s * 0.32
    draw.rectangle([cx - stem_w, cy - s * 0.05, cx + stem_w, base_y + s * 0.7], fill=dark)


def draw_problem_glyph(draw: ImageDraw.ImageDraw) -> None:
    """Punto esclamativo bianco (rettangolo con estremità arrotondate + puntino)."""
    cx, cy = BADGE_CENTER
    s = BADGE_RADIUS * 0.55
    bar_w = s * 0.32
    white = (255, 255, 255, 255)
    draw.rounded_rectangle([cx - bar_w, cy - s, cx + bar_w, cy + s * 0.25], radius=bar_w, fill=white)
    dot_r = bar_w * 1.05
    draw.ellipse([cx - dot_r, cy + s * 0.55 - dot_r, cx + dot_r, cy + s * 0.55 + dot_r], fill=white)


BADGES: dict[str, tuple] = {
    "update": (BADGE_YELLOW, draw_update_glyph),
    "problem": (BADGE_RED, draw_problem_glyph),
}


def compose(base_name: str, base_recolored: Image.Image, badge_name: str | None) -> Image.Image:
    canvas = base_recolored.copy()
    if badge_name is not None:
        fill, glyph_fn = BADGES[badge_name]
        ring = ICON_LIGHT if base_name == "light" else ICON_DARK
        draw = draw_badge_circle(canvas, fill, ring)
        glyph_fn(draw)
    return canvas.resize((FINAL_SIZE, FINAL_SIZE), Image.LANCZOS)


def render_sync_icon(color: tuple[int, int, int]) -> Image.Image:
    """Icona intera per lo stato "in corso": le due frecce circolari a piena
    grandezza (non un badge), lo stesso pittogramma usato da altri client di
    sincronizzazione per restare leggibile alla dimensione reale della tray."""
    canvas = Image.new("RGBA", (WORK_SIZE, WORK_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)
    center = (WORK_SIZE / 2, WORK_SIZE / 2)
    radius = WORK_SIZE * 0.34
    width = WORK_SIZE * 0.095
    draw_circular_arrows(draw, center, radius, width, color)
    return canvas.resize((FINAL_SIZE, FINAL_SIZE), Image.LANCZOS)


def save(image: Image.Image, name: str) -> None:
    rgba_path = OUT_DIR / f"{name}.rgba"
    preview_path = OUT_DIR / f"{name}.preview.png"
    rgba_bytes = image.tobytes("raw", "RGBA")
    expected = FINAL_SIZE * FINAL_SIZE * 4
    assert len(rgba_bytes) == expected, f"{name}: {len(rgba_bytes)} byte, attesi {expected}"
    rgba_path.write_bytes(rgba_bytes)
    image.save(preview_path, format="PNG")
    print(f"scritto {rgba_path.relative_to(ROOT)} ({len(rgba_bytes)} byte)")


def main() -> None:
    source = Image.open(SOURCE_PNG).convert("RGBA")
    if source.size != (WORK_SIZE, WORK_SIZE):
        source = source.resize((WORK_SIZE, WORK_SIZE), Image.LANCZOS)

    bases = {
        "light": recolor(source, ICON_LIGHT),
        "dark": recolor(source, ICON_DARK),
    }

    for base_name, base_image in bases.items():
        for badge_name in (None, "update", "problem"):
            suffix = f"-{badge_name}" if badge_name else ""
            save(compose(base_name, base_image, badge_name), f"tray-{base_name}{suffix}")

    save(render_sync_icon(ICON_LIGHT), "tray-sync-light")
    save(render_sync_icon(ICON_DARK), "tray-sync-dark")


if __name__ == "__main__":
    main()
