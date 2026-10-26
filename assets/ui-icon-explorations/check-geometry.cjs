// Native raster checks: padded-canvas clipping, margins, alpha coverage and mask collisions.
// node check-geometry.cjs <sharp path>
'use strict';
const fs=require('node:fs'),path=require('node:path');
const {icons,reuse}=require('./bold/index.cjs'),{svg}=require('./bold/render.cjs');
const sharp=require(process.argv[2]||'sharp'),root=__dirname;
async function main(){
 const allow=new Set(reuse.map(pair=>[...pair].sort().join('|'))),stats=[],overflow=[],margin=[],masks={};
 let checked=0;
 for(const size of [16,20,24,32,48,64])for(const i of icons){
  const {data,info}=await sharp(Buffer.from(svg(i,size,'mono',true))).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  let partial=0,ink=0,opaque=0,border=0,outside=0;
  const mask=[];
  for(let y=0;y<info.height;y++)for(let x=0;x<info.width;x++){
   const alpha=data[(y*info.width+x)*info.channels+3],lx=x-2,ly=y-2;
   if(lx<0||ly<0||lx>=size||ly>=size)outside=Math.max(outside,alpha);
   else{
    if(alpha){ink++;if(alpha===255)opaque++;else partial++;}
    if(lx===0||ly===0||lx===size-1||ly===size-1)border=Math.max(border,alpha);
    mask.push(alpha>=128?1:0);
   }
  }
  if(outside>16)overflow.push({id:i.id,size,alpha:outside});
  if(border>16)margin.push({id:i.id,size,alpha:border});
  if(size===16)stats.push({id:i.id,ink,opaque,partial,partialFraction:Number((partial/ink).toFixed(4))});
  if(size===16||size===32)masks[`${size}/${i.id}`]=mask;
  checked++;
 }
 const identical=[],near=[];
 for(const size of [16,32])for(let a=0;a<icons.length;a++)for(let b=a+1;b<icons.length;b++){
  const ia=icons[a],ib=icons[b],ma=masks[`${size}/${ia.id}`],mb=masks[`${size}/${ib.id}`];
  let intersection=0,union=0,difference=0;
  for(let p=0;p<ma.length;p++){if(ma[p]&&mb[p])intersection++;if(ma[p]||mb[p])union++;if(ma[p]!==mb[p])difference++;}
  const allowed=allow.has([ia.id,ib.id].sort().join('|'));
  if(!difference)identical.push({a:ia.id,b:ib.id,size,allowed});
  else if(intersection/union>=.88&&!allowed)near.push({a:ia.id,b:ib.id,size,iou:Number((intersection/union).toFixed(4))});
 }
 const partial=stats.map(s=>s.partialFraction).sort((a,b)=>a-b),unexpected=identical.filter(p=>!p.allowed);
 const report={revision:3,checked,criterion:'No significant alpha outside canvas or on outermost pixel; silhouette comparisons threshold alpha at 128/255',overflow,margin,identical,unexpected,near,medianPartialFraction16:partial[Math.floor(partial.length/2)],stats};
 fs.writeFileSync(path.join(root,'geometry-audit.json'),JSON.stringify(report,null,2)+'\n');
 console.log(JSON.stringify({checked,overflow,margin,unexpected,near,medianPartialFraction16:report.medianPartialFraction16}));
 if(overflow.length||margin.length||unexpected.length)process.exitCode=1;
}
main().catch(e=>{console.error(e);process.exitCode=1;});
