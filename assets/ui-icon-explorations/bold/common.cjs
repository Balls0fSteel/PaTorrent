// Shared, intentional action/state reuse. Geometry is authored on the native 16 px grid.
'use strict';
const r=(x,y,w,h,rx=0)=>`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rx}" fill="currentColor"/>`;
const p=d=>`<path d="${d}" fill="currentColor"/>`;
const s=(d,width=2)=>`<path d="${d}" fill="none" stroke="currentColor" stroke-width="${width}" stroke-linecap="butt" stroke-linejoin="miter"/>`;
const c=(x,y,radius)=>`<circle cx="${x}" cy="${y}" r="${radius}" fill="currentColor"/>`;
const o=(x,y,radius,width=2)=>`<circle cx="${x}" cy="${y}" r="${radius}" fill="none" stroke="currentColor" stroke-width="${width}"/>`;
const poly=points=>`<polygon points="${points}" fill="currentColor"/>`;
// Pixel art on the 16 px grid: one string per row, '#' is ink. Each run becomes an
// absolute-coordinate subpath, so at 20/24 px neighbouring rows share rounded edges
// instead of leaving gaps between independently rounded rects.
const px=(x0,y0,rows)=>p(rows.map((row,dy)=>[...row.matchAll(/#+/g)].map(m=>{
 const x=x0+m.index,y=y0+dy;return `M${x} ${y}H${x+m[0].length}V${y+1}H${x}Z`;}).join('')).join(''));
const H={r,p,s,c,o,poly,px};
H.down=p('M7 2h2v7h4l-5 5-5-5h4Z');
H.up=p('M7 14V7H3l5-5 5 5H9v7Z');
H.pause=r(3,2,3,12)+r(10,2,3,12);
H.stop=r(3,3,10,10);
H.check=s('M3 8l3 3 7-7');
H.verify=o(6,6,4)+s('M9 9l5 5')+s('M4 6l1.5 1.5 3-3',1.5);
H.filter=p('M1 2h14v2l-5 5v4l-4 2V9L1 4Z');
// Shared primitives drawn pixel by pixel on the 16 px grid (revisions 4-5).
// Badges are 5 x 5 at x 10-14, y 10-14 (refresh: x 10-14, y 9-14). Bases keep a 1 px
// gap: below y 8 they stay at x <= 8.
// Clockwise refresh badge: a small copy of the full refresh action, an open C with a
// solid corner arrowhead and a two-row gap under it.
H.refreshBadge=px(10,9,[
 '..###',
 '.#.##',
 '#..##',
 '#....',
 '#....',
 '.###.']);
// Radio tower, no dot on top (the dot read as "i"), two arcs a side. The tracker-state
// base is the Trackers-tab tower narrowed (D29): arcs 2 px clear of the mast tip, a tall
// mast and an A-frame lattice, so it reads as a pylon rather than a figure.
H.tower=px(0,0,[
 '',
 '..#.........#',
 '.#..#.....#..#',
 '.#.#..##...#.#',
 '.#.#..##...#.#',
 '.#..#.##..#..#',
 '..#...##....#',
 '......##',
 '.....#..#',
 '.....####',
 '.....#..#',
 '.....#..#',
 '.....#..#',
 '.....#..#']);
H.towerFull=px(0,0,[
 '................',
 '..##........##',
 '.##..........##',
 '.#...##..##...#',
 '.#..##....##..#',
 '.#..#..##..#..#',
 '.#..##.##.##..#',
 '.##..#.##.#..##',
 '..##...##...##',
 '.......##',
 '......####',
 '......####',
 '.....##..##',
 '.....##..##',
 '....##....##']);
H.reannounce=H.tower+H.refreshBadge;
module.exports=H;
