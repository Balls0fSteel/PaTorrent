// Revision 3: one bold family, authored small/large SVG geometry and native pixel exports.
// node build.cjs --sharp <absolute path to sharp>
'use strict';
const fs=require('node:fs'),path=require('node:path');
const {icons,reuse,runtime,toolbar}=require('./bold/index.cjs');
const {svg}=require('./bold/render.cjs');
const root=__dirname,arg=process.argv.indexOf('--sharp');
const sharp=require(arg<0?'sharp':path.resolve(process.argv[arg+1]));
const sizes=[16,20,24,32,48,64],themes=['light','dark','mono'];
const families=[{id:'bold',name:'Bold · pixel fitted',description:'Native-size drawings, intact compound symbols, activity colours and explicit monochrome proofs.'}];
const write=(p,b)=>{const file=path.join(root,p);fs.mkdirSync(path.dirname(file),{recursive:true});fs.writeFileSync(file,b);};
async function main(){
 const manifest={revision:3,families,icons:icons.map(({small,large,...i})=>({...i,largeDrawing:Boolean(large)})),sizes,themes,intendedReuse:reuse,runtime,toolbar};
 write('manifest.json',JSON.stringify(manifest,null,2)+'\n');
 write('manifest.js','window.ICON_EXPLORATIONS = '+JSON.stringify(manifest)+';\n');
 let checked=0;
 // Runtime state and toolbar symbols are rendered first and get their own proof sheet.
 const ordered=[...new Set([...runtime,...toolbar,...icons.map(i=>i.id)])].map(id=>icons.find(i=>i.id===id));
 for(const i of ordered)for(const theme of themes)for(const size of sizes){
  const suffix=theme==='light'?'':`-${theme}`,source=svg(i,size,theme);
  const svgFile=size===16?`svg${suffix}/bold/${i.id}.svg`:`svg${suffix}-size/bold/${size}/${i.id}.svg`;
  write(svgFile,source+'\n');
  const b=await sharp(Buffer.from(source)).png().toBuffer();
  const {data,info}=await sharp(b).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  let ink=0,clear=0;
  for(let j=3;j<data.length;j+=4){if(data[j])ink++;else clear++;}
  if(info.width!==size||info.height!==size||!ink||!clear)throw Error(JSON.stringify({id:i.id,size,theme,ink,clear,info}));
  write(`png${suffix}/bold/${size}/${i.id}.png`,b);checked++;
 }
 write('validation.json',JSON.stringify({revision:3,icons:icons.length,families:1,themes,sizes,svgCount:checked,pngCount:checked,separateLargeDrawings:icons.filter(i=>i.large&&i.large!==i.small).length,checks:['Every PNG decoded at its expected native dimensions','Every PNG contains ink and transparency','Each size rendered from its own pixel-fitted SVG without raster resizing'],limitations:['Artwork only; application integration and platform/DPI checks remain pending','Visual judgments documented separately in QA.md']},null,2)+'\n');
 console.log(JSON.stringify({revision:3,icons:icons.length,svg:checked,png:checked}));
}
main().catch(e=>{console.error(e);process.exitCode=1;});
