# Shipped icons (runtime states + toolbar) at native 16/20/24/32 px, light and dark, 2x nearest.
# Usage: python -I shipped-sizes.py <ui-icon-explorations dir> <output png>   (needs Pillow)
import sys, json
from PIL import Image, ImageDraw, ImageFont
root,out=sys.argv[1],sys.argv[2]
m=json.load(open(root+'/manifest.json'))
ids=m['runtime']+m['toolbar']
font=ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf',12)
sizes=[16,20,24,32]
rowh=140
W=170+sum(s*2+6 for s in sizes)*2
img=Image.new('RGB',(W,len(ids)*rowh//2+10),(200,200,200))
d=ImageDraw.Draw(img)
for k,i in enumerate(ids):
  y=k*rowh//2+4; x=4
  d.text((x,y+8),i,fill=(0,0,0),font=font); x=170
  for th,bg in (('png',(246,246,246)),('png-dark',(32,32,36))):
    for s in sizes:
      im=Image.open(f'{root}/{th}/bold/{s}/{i}.png').convert('RGBA')
      t=Image.new('RGBA',im.size,bg+(255,)); t.alpha_composite(im)
      t=t.resize((s*2,s*2),Image.NEAREST)
      img.paste(t.convert('RGB'),(x,y)); x+=s*2+6
img.save(out)
