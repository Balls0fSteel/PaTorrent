// Actual PNG composites: no interpolation in the native-size proof rows.
// node proof-pages.cjs <sharp path>
'use strict';
const fs=require('node:fs'),path=require('node:path'),sharp=require(process.argv[2]||'sharp');
const root=__dirname,m=require('./manifest.json'),{esc}=require('./bold/render.cjs');
const png=(id,size,theme)=>path.join(root,`png${theme==='light'?'':'-'+theme}/bold/${size}/${id}.png`);
async function sheet(items,name,title){
 const cols=4,cw=300,ch=160,width=cols*cw,height=60+Math.ceil(items.length/cols)*ch;
 let body=`<rect width="${width}" height="${height}" fill="#edf1ee"/><text x="20" y="32" font-family="Segoe UI,sans-serif" font-size="20" fill="#22392e">${esc(title)}</text>`;
 const overlays=[];
 for(let n=0;n<items.length;n++){
  const i=items[n],x=(n%cols)*cw,y=60+Math.floor(n/cols)*ch;
  body+=`<rect x="${x+4}" y="${y}" width="292" height="152" rx="4" fill="#fff"/><text x="${x+14}" y="${y+20}" font-family="Segoe UI,sans-serif" font-size="12" fill="#22392e">${esc(i.id)}</text>`;
  const themes=['light','dark','mono'];
  for(let t=0;t<themes.length;t++){
   const theme=themes[t],tx=x+12+t*94;
   body+=`<rect x="${tx}" y="${y+28}" width="86" height="116" fill="${theme==='dark'?'#111923':'#f3f6f9'}"/><text x="${tx+4}" y="${y+42}" font-family="Segoe UI,sans-serif" font-size="9" fill="${theme==='dark'?'#dce4ef':'#374151'}">${theme} 16 / 24</text>`;
   overlays.push({input:png(i.id,16,theme),left:tx+9,top:y+50},{input:png(i.id,24,theme),left:tx+47,top:y+46});
   overlays.push({input:await sharp(png(i.id,16,theme)).resize(48,48,{kernel:'nearest'}).png().toBuffer(),left:tx+19,top:y+82});
  }
 }
 const bg=await sharp(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${body}</svg>`)).png().toBuffer();
 await sharp(bg).composite(overlays).png().toFile(path.join(root,name));
}
async function main(){
 fs.mkdirSync(path.join(root,'qa/revision3'),{recursive:true});
 for(let start=0;start<m.icons.length;start+=24)await sheet(m.icons.slice(start,start+24),`qa/revision3/all-${String(start/24+1).padStart(2,'0')}.png`,`Bold revision 3 · ${start+1}–${Math.min(start+24,m.icons.length)} of ${m.icons.length} · native 16/24 + 3× nearest zoom`);
 await sheet(m.runtime.map(id=>m.icons.find(i=>i.id===id)),'qa/revision3/runtime.png','The 24 shared runtime state / tracker / network symbols');
 await sheet(m.toolbar.map(id=>m.icons.find(i=>i.id===id)),'preview.png','PaTorrent · bold revision 3 · toolbar artwork');
 // Compact contact sheet, all native 16 px with light/dark/monochrome counterparts.
 const row=29,width=1000,height=70+Math.ceil(m.icons.length/2)*row,overlays=[];
 let body=`<rect width="${width}" height="${height}" fill="#f3f6f9"/><text x="18" y="30" font-family="Segoe UI,sans-serif" font-size="19" fill="#374151">All 131 · actual 16 px · light / dark / monochrome</text>`;
 for(let n=0;n<m.icons.length;n++){
  const i=m.icons[n],x=Math.floor(n/66)*500,y=56+(n%66)*row;
  body+=`<text x="${x+15}" y="${y+13}" font-family="Segoe UI,sans-serif" font-size="11" fill="#374151">${esc(i.id)}</text><rect x="${x+343}" y="${y-2}" width="30" height="22" fill="#111923"/>`;
  for(const [t,theme]of ['light','dark','mono'].entries())overlays.push({input:png(i.id,16,theme),left:x+306+t*44,top:y});
 }
 const bg=await sharp(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${body}</svg>`)).png().toBuffer();
 await sharp(bg).composite(overlays).png().toFile(path.join(root,'small-size-proof.png'));
 // GTK presents a 32 px source at 16 logical px. This sheet explicitly previews that path.
 const gtk=m.runtime.map(id=>m.icons.find(i=>i.id===id)),w=780,h=60+gtk.length*30,parts=[];
 let g=`<rect width="${w}" height="${h}" fill="#f3f6f9"/><text x="18" y="28" font-family="Segoe UI,sans-serif" font-size="18">GTK proof · native 16 / 32 source at 16 logical / native 32</text>`;
 for(let n=0;n<gtk.length;n++){
  const id=gtk[n].id,y=50+n*30;g+=`<text x="18" y="${y+15}" font-family="Segoe UI,sans-serif" font-size="12">${esc(id)}</text>`;
  parts.push({input:png(id,16,'light'),left:340,top:y},{input:await sharp(png(id,32,'light')).resize(16,16).png().toBuffer(),left:440,top:y},{input:png(id,32,'light'),left:550,top:y-5});
 }
 await sharp(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">${g}</svg>`)).composite(parts).png().toFile(path.join(root,'qa/revision3/gtk-logical.png'));
 console.log('Six complete review pages, runtime/toolbar and GTK logical-size proofs rendered.');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
