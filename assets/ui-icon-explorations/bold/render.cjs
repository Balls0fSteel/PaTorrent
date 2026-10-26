// Produce a separate, pixel-fitted SVG drawing at each target size; never resize a PNG.
'use strict';
const attrs=text=>Object.fromEntries([...text.matchAll(/([\w:-]+)="([^"]*)"/g)].map(m=>[m[1],m[2]]));
function fitPath(d,scale,offset=0){
 const ts=d.match(/[a-df-zA-DF-Z]|[-+]?(?:\d*\.?\d+)(?:e[-+]?\d+)?/g)||[];
 const counts={M:2,L:2,H:1,V:1,C:6,S:4,Q:4,T:2,A:7,Z:0};
 let at=0,command='',x=0,y=0,sx=0,sy=0,out=[];
 const q=v=>Math.round(v*scale-offset)+offset;
 while(at<ts.length){
  if(/^[a-z]$/i.test(ts[at]))command=ts[at++];
  const upper=command.toUpperCase(),relative=command!==upper,n=counts[upper];
  if(n===undefined)throw Error('Unsupported SVG path command '+command);
  if(!n){out.push('Z');x=sx;y=sy;command='';continue;}
  const a=ts.slice(at,at+n).map(Number);at+=n;
  if(a.length!==n||a.some(v=>!Number.isFinite(v)))throw Error('Malformed path '+d);
  const ox=relative?x:0,oy=relative?y:0;
  if(upper==='H'){x=a[0]+ox;out.push(`H${q(x)}`);}
  else if(upper==='V'){y=a[0]+oy;out.push(`V${q(y)}`);}
  else if(upper==='A'){
   x=a[5]+ox;y=a[6]+oy;
   out.push(`A${Math.max(.5,Math.round(a[0]*scale*2)/2)} ${Math.max(.5,Math.round(a[1]*scale*2)/2)} ${a[2]} ${a[3]} ${a[4]} ${q(x)} ${q(y)}`);
  }else{
   const b=[];for(let j=0;j<n;j+=2)b.push(q(a[j]+ox),q(a[j+1]+oy));
   x=a[n-2]+ox;y=a[n-1]+oy;out.push(upper+b.join(' '));
   if(upper==='M'){sx=x;sy=y;command=relative?'l':'L';}
  }
 }
 return out.join(' ');
}
function fit(body,size){
 const scale=size/16;
 return body.replace(/<(path|rect|circle|ellipse|polygon|polyline|line)\b([^>]*?)(\/?)>/g,(tag,name,text,closing)=>{
  const a=attrs(text),stroke=a.stroke&&a.stroke!=='none',width=stroke?Math.max(1,Math.round(Number(a['stroke-width']||2)*scale)):0;
  const offset=width%2?.5:0,q=v=>Math.round(Number(v)*scale-offset)+offset;
  if(stroke)a['stroke-width']=width;
  if(a.d)a.d=fitPath(a.d,scale,offset);
  if(a.points)a.points=a.points.trim().split(/[\s,]+/).map(q).join(' ');
  for(const key of ['x','y','x1','x2','y1','y2'])if(key in a)a[key]=q(a[key]);
  for(const key of ['cx','cy'])if(key in a)a[key]=Math.round(Number(a[key])*scale*2)/2;
  for(const key of ['width','height','r','rx','ry'])if(key in a)a[key]=Math.max(Number(a[key])===0?0:.5,Math.round(Number(a[key])*scale));
  if(name==='circle'){const max=Math.min(a.cx-1,a.cy-1,size-1-a.cx,size-1-a.cy)-width/2;a.r=Math.max(.5,Math.min(Math.round(Number(attrs(text).r)*scale*2)/2,max));}
  return '<'+name+' '+Object.entries(a).map(([k,v])=>`${k}="${v}"`).join(' ')+(closing?'/':'')+'>';
 });
}
const palettes={light:{neutral:'#374151',teal:'#006b50',blue:'#1558c0',amber:'#895500',orange:'#ab4200',red:'#bc2335',purple:'#713fbb'},dark:{neutral:'#dce4ef',teal:'#51e0b2',blue:'#83baff',amber:'#ffd17a',orange:'#ffb078',red:'#ff8e9c',purple:'#ceadff'}};
const esc=s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('"','&quot;');
function svg(icon,size,theme='light',padded=false){
 let body=fit(size>=24?(icon.large||icon.small):icon.small,size);
 const color=theme==='mono'?'#000':palettes[theme][icon.color||'neutral'];
 body=body.replaceAll('__COPPER__',theme==='mono'?'#000':theme==='dark'?'#ffc48c':'#9b531d');
 const pad=padded?2:0;
 return `<svg xmlns="http://www.w3.org/2000/svg" width="${size+2*pad}" height="${size+2*pad}" viewBox="${-pad} ${-pad} ${size+2*pad} ${size+2*pad}" role="img" aria-label="${esc(icon.label)}"><title>${esc(icon.label)}</title><g color="${color}">${body}</g></svg>`;
}
module.exports={fit,fitPath,svg,palettes,esc};
