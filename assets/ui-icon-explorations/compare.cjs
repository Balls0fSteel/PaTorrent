const fs=require('fs'),cp=require('child_process'),path=require('path');
const root=__dirname,sharp=require(process.argv[2]||'sharp');
const ids=['add-torrent','add-url','create-torrent','pieces','web-ui','unlock','new-label','force-downloading','no-peers','force-downloading-metadata'];
async function main(){
 const width=1100,height=110+ids.length*64;let b=`<rect width="${width}" height="${height}" fill="#f3f6f9"/>`;
 const t=(x,y,s,size=13)=>`<text x="${x}" y="${y}" font-family="Segoe UI,sans-serif" fill="#374151" font-size="${size}">${s}</text>`;
 b+=t(22,30,'Small-size correction · before / after at exact 16 px',23);
 const families=['bold','outline','tile'];families.forEach((f,j)=>b+=t(245+j*280,65,f+' · before     after'));
 ids.forEach((id,n)=>{const y=95+n*64;b+=t(22,y+15,id);families.forEach((f,j)=>{
  const p=`assets/ui-icon-explorations/png/${f}/16/${id}.png`,old=cp.execFileSync('git',['show','42f7cab:'+p],{cwd:path.join(root,'../..')}),now=fs.readFileSync(path.join(root,`png/${f}/16/${id}.png`));
  for(const [k,img] of [old,now].entries())b+=`<image x="${260+j*280+k*70}" y="${y}" width="16" height="16" href="data:image/png;base64,${img.toString('base64')}"/>`;
 })});
 await sharp(Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${b}</svg>`)).png().toFile(path.join(root,'qa-comparison.png'));
}
main().catch(e=>{console.error(e);process.exitCode=1});
