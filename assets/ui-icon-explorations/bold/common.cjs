// Shared, intentional action/state reuse. Geometry is authored on the native 16 px grid.
'use strict';
const r=(x,y,w,h,rx=0)=>`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="currentColor"/>`;
const p=d=>`<path d="${d}" fill="currentColor"/>`;
const s=(d,width=2)=>`<path d="${d}" fill="none" stroke="currentColor" stroke-width="${width}" stroke-linecap="butt" stroke-linejoin="miter"/>`;
const c=(x,y,radius)=>`<circle cx="${x}" cy="${y}" r="${radius}" fill="currentColor"/>`;
const o=(x,y,radius,width=2)=>`<circle cx="${x}" cy="${y}" r="${radius}" fill="none" stroke="currentColor" stroke-width="${width}"/>`;
const poly=points=>`<polygon points="${points}" fill="currentColor"/>`;
const H={r,p,s,c,o,poly};
H.down=p('M7 2h2v7h4l-5 5-5-5h4Z');
H.up=p('M7 14V7H3l5-5 5 5H9v7Z');
H.pause=r(3,2,3,12)+r(10,2,3,12);
H.stop=r(3,3,10,10);
H.check=s('M3 8l3 3 7-7');
H.verify=o(6,6,4)+s('M9 9l5 5')+s('M4 6l1.5 1.5 3-3',1.5);
H.filter=p('M1 2h14v2l-5 5v4l-4 2V9L1 4Z');
H.announce='<path d="M1 2H8V12H1Z M3 4v1h3V4Z M3 7v1h3V7Z M3 10v1h3v-1Z" fill="currentColor" fill-rule="evenodd"/>'+s('M14 12a2 2 0 1 1-2-2')+p('M12 8l3 3-3 1Z');
module.exports=H;
