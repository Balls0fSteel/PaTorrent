// Optional asset export helper. Requires Node.js and sharp; no application dependency.
// node build.cjs [--sharp /path/to/sharp]
const fs = require('node:fs/promises');
const path = require('node:path');
const sharpArg = process.argv.indexOf('--sharp');
const sharp = require(sharpArg >= 0 ? path.resolve(process.argv[sharpArg + 1]) : 'sharp');
const root = __dirname;
const load = async p => JSON.parse(await fs.readFile(path.join(root, p), 'utf8'));
const esc = s => String(s).replace(/[&<>\"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','\"':'&quot;'}[c]));
const rounded = (x,y,w,h,fill,stroke='none',r=18) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${r}" fill="${fill}" stroke="${stroke}"/>`;
const text = (x,y,s,size=18,color='#1B3731',weight=400) => `<text x="${x}" y="${y}" font-family="Segoe UI,Arial,sans-serif" font-size="${size}" font-weight="${weight}" fill="${color}">${esc(s)}</text>`;
const svg = (w,h,body) => Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">${body}</svg>`);
const save = async (rel,data) => {const p=path.join(root,rel);await fs.mkdir(path.dirname(p),{recursive:true});await fs.writeFile(p,data);};
const render = async (file,size) => sharp(path.join(root,file),{density:Math.max(72,72*size/16)}).resize(size,size,{kernel:'lanczos3'}).png().toBuffer();
const sizes = [16,20,24,32,48,64,128,256];

function ico(frames) {
  const header=Buffer.alloc(6+frames.length*16);
  header.writeUInt16LE(1,2);header.writeUInt16LE(frames.length,4);
  let offset=header.length;
  frames.forEach(({size,png},i)=>{
    const p=6+i*16;
    header[p]=size===256?0:size;header[p+1]=header[p];
    header.writeUInt16LE(1,p+4);header.writeUInt16LE(32,p+6);
    header.writeUInt32LE(png.length,p+8);header.writeUInt32LE(offset,p+12);
    offset+=png.length;
  });
  return Buffer.concat([header,...frames.map(f=>f.png)]);
}

async function verifyPng(buffer,size,label) {
  const {data,info}=await sharp(buffer).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  if(info.width!==size||info.height!==size||info.channels!==4)throw Error(`${label}: dimensions`);
  let ink=0,transparent=0;
  for(let p=3;p<data.length;p+=4){if(data[p]>0)ink++;if(data[p]===0)transparent++;}
  if(!ink||!transparent)throw Error(`${label}: empty or opaque`);
  const corners=[3,(size-1)*4+3,(size*(size-1))*4+3,(size*size-1)*4+3];
  if(corners.some(p=>data[p]!==0))throw Error(`${label}: opaque corner`);
}

async function main(){
  const manifest=await load('manifest.json');
  const states=await load('states/manifest.json');
  const report={candidates:[],stateIcons:states.icons.length,appPngSizes:manifest.appPngSizes,appIcoSizes:sizes,statePngSizes:manifest.statePngSizes,checks:[]};
  for(const c of manifest.candidates){
    const source=await sharp(path.join(root,c.source)).metadata();
    if(!source.hasAlpha||source.width!==source.height)throw Error(`${c.id}: invalid master`);
    const frames=[];
    for(const size of manifest.appPngSizes){
      const png=await render(c.source,size);await verifyPng(png,size,`${c.id}/${size}`);
      await save(`exports/app/${c.id}/${size}.png`,png);
      if(sizes.includes(size))frames.push({size,png});
    }
    const data=ico(frames);
    if(data.readUInt16LE(2)!==1||data.readUInt16LE(4)!==sizes.length)throw Error('ICO header');
    for(let i=0;i<frames.length;i++){
      const p=6+i*16;const offset=data.readUInt32LE(p+12);const length=data.readUInt32LE(p+8);
      await verifyPng(data.subarray(offset,offset+length),frames[i].size,`${c.id}/ico/${i}`);
    }
    await save(`exports/app/${c.id}.ico`,data);
    report.candidates.push({id:c.id,masterSize:source.width,alpha:true,icoFrames:frames.length});
  }
  for(const icon of states.icons){
    const xml=await fs.readFile(path.join(root,'states',icon.filename),'utf8');
    if(!/viewBox="0 0 16 16"/.test(xml)||!/<title\b/.test(xml)||!/<desc\b/.test(xml))throw Error(`${icon.id}: SVG metadata`);
    if(/<script\b|<image\b|<foreignObject\b|(?:href|url\s*\()/i.test(xml))throw Error(`${icon.id}: external content`);
    for(const size of manifest.statePngSizes){
      const png=await render(`states/${icon.filename}`,size);await verifyPng(png,size,`${icon.id}/${size}`);
      await save(`exports/states/${size}/${icon.id}.png`,png);
    }
  }
  report.checks.push('All app and state PNGs have visible content and transparent corners.','Every ICO has eight independently decoded PNG frames.','Every state SVG uses a 16-unit view box and accessible title/description, without scripts or external resources.');
  // Fixed-layout comparison sheets; the icon masters remain unchanged.
  let bg=rounded(0,0,1680,900,'#F4F1E9','none',0);
  bg+=text(64,68,'PATINATORRENT  /  ICON EXPLORATIONS',16,'#68776F',600);
  bg+=text(64,132,'Copper, enamel & a little character.',44,'#173D35',600);
  bg+=text(64,174,'Three original directions for a compact desktop torrent client.',20,'#68776F');
  const overlays=[];
  for(let i=0;i<manifest.candidates.length;i++){
    const c=manifest.candidates[i],x=64+i*528;
    bg+=rounded(x,218,496,594,'#FFFEFA','#DBDFD4');
    bg+=text(x+28,264,`0${i+1}   ${c.name}`,27,'#173D35',600);
    const sub=['The name becomes the symbol.','Two-way flow, one simple loop.','A durable tool, built in Rust.'][i];
    bg+=text(x+28,296,sub,18,'#68776F');
    overlays.push({input:await render(c.source,248),left:x+124,top:322});
    bg+=text(x+28,614,'ACTUAL PIXEL SIZES',12,'#68776F',600);
    bg+=rounded(x+22,632,452,62,'#F0F2ED','none',10);
    bg+=rounded(x+22,708,452,62,'#172C29','none',10);
    let pos=x+44;
    for(const size of [16,24,32,48,64]){
      overlays.push({input:await render(c.source,size),left:pos,top:632+Math.floor((62-size)/2)});
      overlays.push({input:await render(c.source,size),left:pos,top:708+Math.floor((62-size)/2)});
      bg+=text(pos+Math.max(0,Math.floor(size/2)-7),794,String(size),12,'#68776F');
      pos+=size+29;
    }
    bg+=text(x+426,794,'px',12,'#68776F');
  }
  bg+=text(64,860,'Transparent PNG masters  •  Windows ICO: 16–256 px  •  PNG exports: 16–512 px',17,'#68776F');
  await save('app-candidates.png',await sharp(svg(1680,900,bg)).composite(overlays).png().toBuffer());

  let sbg=rounded(0,0,1680,1240,'#F4F1E9','none',0);
  sbg+=text(64,65,'PATINATORRENT  /  STATE ICONS',16,'#68776F',600);
  sbg+=text(64,123,'Small icons. Clear states.',42,'#173D35',600);
  sbg+=text(64,164,'30 editable vectors · 16 px grid · light and dark proofs · shape as well as colour',19,'#68776F');
  const so=[];
  for(let i=0;i<states.icons.length;i++){
    const icon=states.icons[i],x=64+(i%5)*314,y=202+Math.floor(i/5)*162;
    sbg+=rounded(x,y,296,146,'#FFFEFA','#DBDFD4',12);
    so.push({input:await render(`states/${icon.filename}`,40),left:x+18,top:y+20});
    const label=icon.label.replace('[F] ','Forced ').replace('Queued for checking (resume data)','Queued resume check').replace('Queued for checking (files)','Queued file check');
    const words=label.split(' '),lines=[''];
    for(const w of words){if((lines[lines.length-1]+w).length>22)lines.push('');lines[lines.length-1]+=(lines[lines.length-1]?' ':'')+w;}
    lines.forEach((line,j)=>sbg+=text(x+74,y+30+j*22,line,16,'#173D35',600));
    sbg+=text(x+74,y+77,icon.group.toUpperCase(),10,'#68776F',600);
    sbg+=rounded(x+10,y+94,276,42,'#172C29','none',8);
    for(const [j,size] of [16,24,32].entries()){
      so.push({input:await render(`states/${icon.filename}`,size),left:x+24+j*48,top:y+94+Math.floor((42-size)/2)});
    }
    sbg+=text(x+186,y+120,'16 / 24 / 32 px',11,'#AEBFB7');
  }
  sbg+=text(64,1210,'Optional icons reserve future states; the current application does not yet detect those conditions.',16,'#68776F');
  await save('state-icons.png',await sharp(svg(1680,1240,sbg)).composite(so).png().toBuffer());
  await save('validation.json',JSON.stringify(report,null,2)+'\n');
  const template=await fs.readFile(path.join(root,'review.template.html'),'utf8');
  const payload=JSON.stringify({manifest,states}).replace(/</g,'\\u003c');
  await save('review.html',template.replace('/* ICON_DATA */',`const DATA = ${payload};`));
  console.log(JSON.stringify(report,null,2));
}
main().catch(error=>{console.error(error);process.exitCode=1;});
