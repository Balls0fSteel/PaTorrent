// Render paginated proof sheets at exact raster sizes. node proof-pages.cjs <sharp path>
const fs=require('fs'),path=require('path'),sharp=require(process.argv[2]||'sharp');
const root=__dirname,m=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));
const esc=s=>s.replaceAll('&','&amp;').replaceAll('<','&lt;');
async function main(){
 fs.mkdirSync(path.join(root,'qa'),{recursive:true});
 for(const size of [16,24,32])for(let start=0;start<m.icons.length;start+=27){
  const subset=m.icons.slice(start,start+27),width=1100,row=40,height=90+subset.length*row;
  let b=`<rect width="1100" height="${height}" fill="#f3f6f9"/><rect x="550" width="550" height="${height}" fill="#111923"/>`;
  for(let side=0;side<2;side++){
   const color=side?'#dce4ef':'#374151',ox=550*side;
   const text=(x,y,t,s=12)=>`<text x="${ox+x}" y="${y}" fill="${color}" font-family="Segoe UI,sans-serif" font-size="${s}">${esc(t)}</text>`;
   b+=text(18,25,`Revision 2 · ${size} px · ${side?'dark':'light'} · ${start+1}–${start+subset.length}`,17);
   m.families.forEach((f,j)=>b+=text(240+j*100,52,f.name,10));
   subset.forEach((i,n)=>{
    const y=72+n*row;b+=text(18,y+18,i.label,11);
    m.families.forEach((f,j)=>{
     const png=fs.readFileSync(path.join(root,`png${side?'-dark':''}/${f.id}/${size}/${i.id}.png`));
     b+=`<image x="${ox+255+j*100}" y="${y}" width="${size}" height="${size}" href="data:image/png;base64,${png.toString('base64')}"/>`;
    });
   });
  }
  await sharp(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${b}</svg>`)).png().toFile(path.join(root,`qa/${size}-${String(start/27+1).padStart(2,'0')}.png`));
 }
 console.log('15 complete native-size proof sheets rendered');
}
main().catch(e=>{console.error(e);process.exitCode=1});
