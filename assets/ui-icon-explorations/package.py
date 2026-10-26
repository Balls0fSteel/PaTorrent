"""Bundle only the active bold artwork and its editable sources/proofs."""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
import json

root = Path(__file__).resolve().parent
repo = root.parent.parent
manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
files = {root / name for name in ("README.md", "QA.md", "manifest.json", "manifest.js", "inventory.json", "build.cjs", "check-geometry.cjs", "proof-pages.cjs", "package.py", "review.html", "validation.json", "geometry-audit.json", "preview.png", "small-size-proof.png", "app-concepts.png", "prompts.json", "review-2026-10-26/REPORT.md")}
files.update((root / "bold").glob("*.cjs"))
files.update((root / "qa/revision3").glob("*.png"))
for theme in ("", "-dark", "-mono"):
    for icon in manifest["icons"]:
        name = icon["id"]
        files.add(root / f"svg{theme}/bold/{name}.svg")
        for size in manifest["sizes"]:
            files.add(root / f"png{theme}/bold/{size}/{name}.png")
            if size != 16:
                files.add(root / f"svg{theme}-size/bold/{size}/{name}.svg")
output = repo / "dist/artwork/patorrent-bold-revision3.zip"
output.parent.mkdir(parents=True, exist_ok=True)
with ZipFile(output, "w", compression=ZIP_DEFLATED, compresslevel=9) as archive:
    for file in sorted(files):
        info = ZipInfo(file.relative_to(root).as_posix())
        info.compress_type = ZIP_DEFLATED
        archive.writestr(info, file.read_bytes())
    license_info = ZipInfo("LICENSE.md")
    license_info.compress_type = ZIP_DEFLATED
    archive.writestr(license_info, (repo / "LICENSE.md").read_bytes())
print(f"{output}: {len(files) + 1} files, {output.stat().st_size:,} bytes")
