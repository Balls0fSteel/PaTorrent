from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import zipfile
BASE=Path(__file__).resolve().parent
SIZES=[16,20,24,32,40,48,64,96,128,256]
font=ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf",18)
smallfont=ImageFont.truetype("C:/Windows/Fonts/segoeui.ttf",15)
rows=[]
old=BASE.parent/"rookparcel-candidates-2026-11-05/ico/08-postal-rook-stamp.ico"
with Image.open(old) as im:
    rows.append(("Original stamp",{s:im.ico.getimage((s,s)).convert("RGBA") for s in SIZES}))
for src in sorted((BASE/"sources").glob("*.png")):
    name=src.stem
    out=BASE/"icons"/name
    out.mkdir(parents=True,exist_ok=True)
    with Image.open(src) as source:
        master=source.convert("RGBA")
        frames={s:master.resize((s,s),Image.Resampling.LANCZOS) for s in SIZES}
        for s,frame in frames.items(): frame.save(out/f"{s}.png")
        master.save(BASE/"icons"/f"{name}.ico",format="ICO",sizes=[(s,s) for s in SIZES])
    with Image.open(BASE/"icons"/f"{name}.ico") as ico:
        assert ico.ico.sizes()=={(s,s) for s in SIZES}
        for s in SIZES: ico.ico.getimage((s,s)).load()
    rows.append((name,frames))
# Actual-size frames are pasted without scaling. Enlargements are nearest-neighbor.
w=1020; rh=154
sheet=Image.new("RGB",(w,85+len(rows)*rh),(239,241,244))
d=ImageDraw.Draw(sheet)
d.text((20,12),"Postal Rook — actual small-size comparison",font=font,fill="#142330")
for x,t in [(260,"16"),(315,"20"),(370,"24"),(430,"32"),(505,"48"),(585,"16px enlarged 8x"),(790,"128px")]:
    d.text((x,53),t,font=smallfont,fill="#142330")
for i,(name,frames) in enumerate(rows):
    y=85+i*rh
    d.text((20,y+56),name,font=font,fill="#142330")
    for x,s in [(260,16),(315,20),(370,24),(430,32),(505,48)]:
        frame=frames[s]
        sheet.paste(frame,(x,y+22),frame)
        d.rectangle((x-4,y+83,x+s+4,y+83+s+8),fill="#18212b")
        sheet.paste(frame,(x,y+87),frame)
    large=frames[16].resize((128,128),Image.Resampling.NEAREST)
    sheet.paste(large,(590,y+7),large)
    sheet.paste(frames[128],(790,y+7),frames[128])
    d.line((20,y+rh-1,1000,y+rh-1),fill="#ccd2d8")
sheet.save(BASE/"comparison.png")
with zipfile.ZipFile(BASE/"postal-rook-small-icons.zip","w",zipfile.ZIP_DEFLATED) as z:
    for p in sorted((BASE/"icons").glob("*.ico")): z.write(p,p.name)
    z.write(BASE/"comparison.png","comparison.png")
print(f"Verified {len(rows)-1} ICOs, {(len(rows)-1)*len(SIZES)} frames")

# Compact shortlist: actual sizes remain unscaled.
chosen=[rows[i] for i in (0,9,10,11,5)]
proof=Image.new("RGB",(800,70+len(chosen)*112),(239,241,244))
pd=ImageDraw.Draw(proof)
pd.text((16,10),"Postal Rook shortlist — open at 100% for native sizes",font=font,fill="#142330")
for x,t in [(245,"16px"),(315,"24px"),(385,"32px"),(470,"16px at 4x"),(640,"64px")]:
    pd.text((x,43),t,font=smallfont,fill="#142330")
for i,(name,frames) in enumerate(chosen):
    y=70+i*112
    pd.text((16,y+36),name,font=font,fill="#142330")
    for x,s in [(245,16),(315,24),(385,32)]:
        frame=frames[s]
        proof.paste(frame,(x,y+12),frame)
        pd.rectangle((x-3,y+58,x+s+3,y+64+s),fill="#18212b")
        proof.paste(frame,(x,y+61),frame)
    frame=frames[16].resize((64,64),Image.Resampling.NEAREST)
    proof.paste(frame,(480,y+14),frame)
    proof.paste(frames[64],(640,y+14),frames[64])
    pd.line((16,y+111,780,y+111),fill="#ccd2d8")
proof.save(BASE/"shortlist.png")
with zipfile.ZipFile(BASE/"postal-rook-small-icons.zip","a",zipfile.ZIP_DEFLATED) as z:
    z.write(BASE/"shortlist.png","shortlist.png")
    if (BASE/"README.md").exists(): z.write(BASE/"README.md","README.md")

