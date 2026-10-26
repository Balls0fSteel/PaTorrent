const fs=require('fs'),path=require('path');
const root=__dirname,sharp=require(process.argv[2]||'sharp');
async function main(){
 const m=JSON.parse(fs.readFileSync(path.join(root,'manifest.json')));
 let overflow=[];
 for(const theme of ['svg','svg-dark'])for(const f of m.families)for(const i of m.icons){
  const p=`${theme}/${f.id}/${i.id}.svg`;
  let s=fs.readFileSync(path.join(root,p),'utf8').replace('width="16" height="16" viewBox="0 0 16 16"','width="20" height="20" viewBox="-2 -2 20 20"');
  const {data,info}=await sharp(Buffer.from(s)).ensureAlpha().raw().toBuffer({resolveWithObject:true});
  let max=0;for(let y=0;y<20;y++)for(let x=0;x<20;x++)if(x<2||y<2||x>=18||y>=18)max=Math.max(max,data[(y*20+x)*info.channels+3]);
  if(max>16)overflow.push({path:p,alpha:max});
 }
 fs.writeFileSync(path.join(root,'geometry-audit.json'),JSON.stringify({checked:m.icons.length*6,criterion:'No significant stroke alpha beyond the 16-unit canvas (alpha >16/255)',overflow},null,2)+'\n');
 console.log(JSON.stringify({overflow})); if(overflow.length)process.exitCode=1;
}
main().catch(e=>{console.error(e);process.exitCode=1});
