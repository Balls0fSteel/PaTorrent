"""Turns a pixel map (one character per pixel) into a crisp SVG of whole-pixel rectangles.

    python pixelmap.py src/mark-16.txt src/mark-16.svg

The map's first line is the size; each following line is one row. Characters:
  . transparent   R stamp red    r dark red     C paper cream   N rook navy
  W wing navy     G pale rook face Y badge gold  K kraft         L kraft top   k kraft side
"""
import sys

COLORS = {
    "R": "#B8352A", "r": "#8E271F", "C": "#F7EEDC", "N": "#14233D", "W": "#273A5C",
    "G": "#C9CFD8", "Y": "#F2C14E", "K": "#C4864A", "L": "#DDA868", "k": "#9C6630",
}


def main(src: str, dst: str) -> None:
    lines = [l.rstrip("\n") for l in open(src, encoding="utf-8") if l.strip() and not l.startswith("#")]
    size = int(lines[0])
    rows = lines[1:]
    assert len(rows) == size and all(len(r) == size for r in rows), "map is not square"
    rects = []
    for y, row in enumerate(rows):
        x = 0
        while x < size:
            c = row[x]
            run = 1
            while x + run < size and row[x + run] == c:
                run += 1
            if c != ".":
                rects.append(f'<rect x="{x}" y="{y}" width="{run}" height="1" fill="{COLORS[c]}"/>')
            x += run
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}" '
        'shape-rendering="crispEdges">\n'
        f"  <title>Rookparcel</title>\n  <desc>The rook's head in its postal cap with a parcel, drawn pixel by pixel for {size} px.</desc>\n  "
        + "\n  ".join(rects)
        + "\n</svg>\n"
    )
    open(dst, "w", encoding="utf-8", newline="\n").write(svg)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
