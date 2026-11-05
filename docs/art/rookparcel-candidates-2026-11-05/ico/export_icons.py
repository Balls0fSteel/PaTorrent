from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import json, zipfile
from collections import deque

ROOT=Path(r"C:\Users\x\Documents\dev\foss\libretorrent")
BASE=ROOT/"docs/art/rookparcel-candidates-2026-11-05"
OUT=BASE/"ico"
OUT.mkdir(exist_ok=True)
SIZES=[16,20,24,32,40,48,64,96,128,256]
# Pixel coordinates on the original generated sheets, selected by visual inspection.
CROPS={
"01-boxcastle": [(593,585,846,838),(898,585,1151,838)],
"02-courier-rook": [(264,631,533,900),(617,631,887,900)],
"03-swarm-tower": [(127,719,417,1009),(528,720,813,1009)],
"04-parcel-wing": [(110,650,427,967),(486,650,803,967)],
"05-hidden-rook": [(145,695,443,993),(520,695,818,993)],
"06-night-dispatch": [(185,720,463,995),(565,720,844,995)],
"07-verdigris-tower": [(588,623,823,858),(856,623,1091,858)],
"08-postal-rook": [(591,642,844,914),(862,642,1115,914)],
"09-origami-courier": [(193,638,479,924),(591,638,878,924)],
"10-pixel-rook": [(609,551,874,836),(896,550,1165,838)],
"11-orbital-parcel": [(167,610,472,916),(575,610,880,916)],
"12-rooklet": [(43,647,418,1022),(499,647,874,1022)]
}
entries=[]
def save_icon(name, image):
    canvas=Image.new("RGBA",(512,512))
    scale=480/max(image.size)
    image=image.resize((round(image.width*scale),round(image.height*scale)),Image.Resampling.LANCZOS)
    canvas.alpha_composite(image,((512-image.width)//2,(512-image.height)//2))
    canvas.save(OUT/(name+".png"))
    canvas.save(OUT/(name+".ico"),format="ICO",sizes=[(s,s) for s in SIZES])
    with Image.open(OUT/(name+".ico")) as check:
        assert check.ico.sizes()=={(s,s) for s in SIZES}, name
        for s in SIZES:
            check.ico.getimage((s,s)).load()
    entries.append((name,canvas))
for name,rects in CROPS.items():
    with Image.open(BASE/(name+".png")) as im:
        for variant,rect in zip(("light","dark"),rects):
            assert 0<=rect[0]<rect[2]<=im.width and 0<=rect[1]<rect[3]<=im.height,(name,im.size,rect)
            crop=im.crop(rect).convert("RGBA")
            # Clip the tile to its rounded silhouette, removing surrounding sheet and shadow.
            mask=Image.new("L",crop.size)
            ImageDraw.Draw(mask).rounded_rectangle((3,3,crop.width-4,crop.height-4),radius=round(crop.width*.20),fill=255)
            crop.putalpha(mask)
            save_icon(name+"-"+variant,crop)
with Image.open(BASE/"08-postal-rook.png") as im:
    stamp=im.crop((28,180,559,817)).convert("RGBA")
    # Remove only edge-connected pale sheet background; retain enclosed cream stamp paper.
    pix=stamp.load(); w,h=stamp.size
    seen=set(); queue=deque()
    for x in range(w): queue.extend(((x,0),(x,h-1)))
    for y in range(h): queue.extend(((0,y),(w-1,y)))
    while queue:
        x,y=queue.popleft()
        if (x,y) in seen: continue
        seen.add((x,y))
        r,g,b,a=pix[x,y]
        if min(r,g,b)<160 or max(r,g,b)-min(r,g,b)>80: continue
        pix[x,y]=(r,g,b,0)
        for xx,yy in ((x-1,y),(x+1,y),(x,y-1),(x,y+1)):
            if 0<=xx<w and 0<=yy<h and (xx,yy) not in seen: queue.append((xx,yy))
    save_icon("08-postal-rook-stamp",stamp)
# Review grid shows the exported assets and their actual 16/32/48 pixel frames.
font=ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf",17)
grid=Image.new("RGB",(1250,7*236),(226,229,233))
d=ImageDraw.Draw(grid)
for i,(name,canvas) in enumerate(entries):
    x=(i%4)*312; y=(i//4)*236
    preview=canvas.resize((150,150),Image.Resampling.LANCZOS)
    grid.paste(preview,(x+10,y+30),preview)
    d.text((x+10,y+6),name,fill=(20,25,30),font=font)
    with Image.open(OUT/(name+".ico")) as icon:
        for s,yy in ((16,50),(32,83),(48,133)):
            frame=icon.ico.getimage((s,s))
            grid.paste(frame,(x+180,y+yy),frame)
            d.text((x+238,y+yy),str(s),fill=(20,25,30),font=font)
grid.save(OUT/"preview.png")
(OUT/"crops.json").write_text(json.dumps(CROPS,indent=2)+"\n")
(OUT/"README.md").write_text("# Rookparcel ICO candidates\n\n25 ICO files: light and dark tile crops for all 12 concepts, plus a transparent standalone Postal Rook stamp. Postal Rook is the owner's preferred direction.\n\nEach ICO contains 16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 pixel frames. Matching 512 px transparent PNG canvases are included. Tile corners are clipped to rounded silhouettes. The stamp retains its cream paper with transparent outer perforations.\n\nThese are crops of the generated artwork, resampled with Lanczos, not pixel-redrawn small icons. At 16 px the detailed postal bird becomes indistinct; dedicated small-size refinement remains useful. Production application resources have not been replaced.\n\n![Export review](preview.png)\n\nSee export_icons.py and crops.json for reproducible extraction coordinates.\n",encoding="utf-8")
with zipfile.ZipFile(BASE/"rookparcel-ico-candidates.zip","w",zipfile.ZIP_DEFLATED) as archive:
    for p in sorted(OUT.glob("*.ico")): archive.write(p,p.name)
    archive.write(OUT/"README.md","README.md")
    archive.write(OUT/"preview.png","preview.png")
print(f"Exported and decoded {len(entries)} ICO files, {len(entries)*len(SIZES)} frames.")

