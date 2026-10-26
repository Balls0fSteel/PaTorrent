'use strict';
// Revision 3: native 16px geometry. Orthogonal edges are on the pixel grid.
// Every body declares its ink; currentColor supplies only the semantic palette.
module.exports = (H) => {
  const {r,p,s,c,o,poly} = H;
  const even=d=>`<path d="${d}" fill="currentColor" fill-rule="evenodd"/>`;
  const result = {};
  const add = (id, small, color='neutral', large) => {result[id]={small,color,...(large?{large}:{})};};
  const frame=(x=2,y=2,w=12,h=12)=>s(`M${x} ${y}h${w}v${h}H${x}Z`);
  const page=s('M3 2h6l4 4v8H3ZM9 2v4h4');
  const shield=s('M8 2l5 2v4c0 2.5-3 4.5-5 5-2-1-5-2.5-5-5V4Z');
  const compactRss=c(2,8,1)+s('M2 5a3 3 0 0 1 3 3')+s('M2 2a6 6 0 0 1 6 6');
  const fullRss=c(3,13,1.5)+s('M2 7a7 7 0 0 1 7 7')+s('M2 2a12 12 0 0 1 12 12');
  add('search',o(6,6,4)+p('M9 10l1-1 5 5-1 1Z'));
  add('filter',H.filter);
  // Revision 4: a 2 x 2 grid of solid tiles, unlike copy's overlapping squares.
  add('all-torrents',r(2,2,5,5)+r(9,2,5,5)+r(2,9,5,5)+r(9,9,5,5),'teal');
  add('active',p('M1 7h3l2-4 3 8 2-4h4v2h-3l-3 6-3-8-1 2H1Z'),'teal');
  // The active pulse flattened to a line with one small bump.
  add('inactive',H.px(0,5,['.......##','......####','.######..######','.#####....#####']));
  add('completed',H.check,'teal');
  add('labels',even('M1 1h7l7 7-7 7-7-7Z M3 3h4l5 5-4 4-5-5Z')+c(5,5,1),'purple');
  add('rss',fullRss,'orange');
  // A complete RSS base and separate funnel; the modifier never replaces an arc.
  add('rss-rule',compactRss+p('M9 9h6v1l-2 2v2l-2 1v-3l-2-2Z'),'orange',
    c(2,8,1)+s('M2 5a3 3 0 0 1 3 3',1.5)+s('M2 2a6 6 0 0 1 6 6',1.5)+p('M9 9h6v1l-2 2v2l-2 1v-3l-2-2Z'));
  add('statistics',r(2,9,3,5)+r(7,6,3,8)+r(12,2,3,12),'blue');
  add('speed',s('M2 2v12h12')+s('M4 10l3-4 3 2 4-5'),'teal');
  add('general',o(8,8,6)+r(7,7,2,5)+r(7,4,2,2),'blue');
  // Radio pylon with broadcast arcs above its apex (D30), matching the tracker states.
  add('trackers',H.towerFull,'purple');
  add('peers',c(5,5,2)+s('M2 14v-2a3 3 0 0 1 6 0v2')+c(12,6,1.5)+s('M11 10a3 3 0 0 1 3 3v1'),'blue');
  // A piece map: received pieces solid, missing ones hollow. Distinct from all-torrents.
  add('pieces',H.px(0,2,['..###.###.###','..###.###.###','..###.###.###','','..###.###.###','..###.###.#.#','..###.###.###','','..###.###.###','..###.#.#.#.#','..###.###.###']),'teal');
  add('files',page,'amber');
  // Terminal window with a ">_" prompt (revision 2's reading, redrawn on the grid).
  add('logger',H.px(0,2,['..############','.##############','.##..........##','.##.#........##','.##.##.......##','.##..##......##','.##.##.......##','.##.#..####..##','.##....####..##','.##..........##','.##############','..############']));
  add('ratings',p('M8 1l2 4 5 1-4 4 1 5-4-3-4 3 1-5-4-4 5-1Z'),'amber');
  // Six substantial teeth and a six-pixel clear centre.
  const gearPoints=[];
  for(let tooth=0;tooth<6;tooth++)for(const [angle,radius] of [[-22,5],[-12,7],[12,7],[22,5]]){
    const rad=(-90+tooth*60+angle)*Math.PI/180;
    gearPoints.push([8+radius*Math.cos(rad),8+radius*Math.sin(rad)].map(v=>Math.round(v*2)/2));
  }
  const gearOutline='M'+gearPoints.map(pt=>pt.join(' ')).join('L')+'Z';
  const gear=`<path d="${gearOutline}M11 8a3 3 0 1 0-6 0a3 3 0 1 0 6 0Z" fill="currentColor" fill-rule="evenodd"/>`;
  add('preferences',gear);
  add('connection',s('M3 2h10v8H3ZM5 2v4M8 2v4M11 2v4M6 10v3h4v-3M8 13v2'),'blue');
  add('bandwidth',p('M3 14V6H1l3-4 3 4H5v8ZM11 2h2v8h2l-3 4-3-4h2Z'),'teal');
  add('bittorrent',s('M4 4l8 0M4 5l3 7M12 5l-3 7',1.5)+c(3,3,2)+c(13,3,2)+c(8,13,2)+c(8,6,1),'purple',
    s('M4 4l8 0M4 5l3 7M12 5l-3 7',1.5)+o(3,3,1.5,1.5)+o(13,3,1.5,1.5)+o(8,13,1.5,1.5)+c(8,6,1));
  add('directories',s('M2 3h4l2 2h5v7H2ZM5 14h9V7'),'amber');
  add('scheduler',s('M2 4h12v10H2ZM5 1v5M11 1v5M2 7h12')+r(4,9,3,3)+r(9,9,3,3),'purple');
  add('remote',s('M2 2h5v12H2Z')+r(3,11,3,1)+s('M10 7a2 2 0 0 1 0 3M11 4a5 5 0 0 1 0 8',1.5),'blue',
    s('M2 2h5v12H2Z',1.5)+r(3,11,3,1)+s('M10 7a2 2 0 0 1 0 3M11 4a5 5 0 0 1 0 8',1.5));
  add('web-ui',s('M2 2h12v5M2 2v12h4M2 5h12')+r(4,3,1,1)+o(10.5,10.5,3.5,1.5)+s('M7 10.5h7M10.5 7c-2 2-2 5 0 7M10.5 7c2 2 2 5 0 7',1),'blue',
    s('M2 2h12v5M2 2v12h4M2 5h12',1.5)+r(4,3,1,1)+r(6,3,1,1)+o(10.5,10.5,3.5,1.5)+s('M7 10.5h7M10.5 7c-2 2-2 5 0 7M10.5 7c2 2 2 5 0 7',1));
  // One readable letter in a speech bubble, rather than two tiny scripts.
  add('language',even('M1 1h14v11H7l-4 3v-3H1Z M3 3v7h2v1l2-1h6V3Z')+s('M6 9l2-5 2 5M7 7h2',1.5),'purple',
    even('M1 1h14v11H7l-4 3v-3H1Z M2.5 2.5v8h2v1.5l2.5-1.5h6.5v-8Z')+s('M6 9l2-5 2 5M7 7h2',1.5));
  add('appearance',o(8,8,6)+p('M8 2a6 6 0 0 1 0 12Z'),'purple');
  add('advanced',r(2,3,12,2)+r(2,11,12,2)+r(5,1,3,6)+r(10,9,3,6));
  add('privacy',shield+s('M5 8l2 2 4-4'),'teal');
  // Client -> relay -> server: two small nodes linked through a larger hollow relay.
  add('proxy',H.px(0,4,['.....######','.....######','.###.##..##.###','.######..######','.###.##..##.###','.....######','.....######']),'blue');
  add('disk',s('M2 3h12v10H2Z')+r(4,5,8,2)+r(4,10,3,1)+r(11,10,1,1),'amber');
  add('notifications',even('M7 1h2v2a4 4 0 0 1 3 4v4l2 2H2l2-2V7a4 4 0 0 1 3-4Z M6 7v5h4V7a2 2 0 0 0-4 0Z')+r(6,14,4,1),'amber');
  add('power',s('M4 4a5 5 0 1 0 8 0')+r(7,1,2,7),'red');
  // A ring of six nodes joined by 1 px links; no frame.
  add('dht',H.px(0,0,['','.......##','.......##','.....#....#','.##.#......#.##','.###........###','..#..........#','..#..........#','..#..........#','..#..........#','.###........###','.##.#......#.##','.....#....#','.......##','.......##']),'blue');
  add('upload',H.up,'blue');
  add('download',H.down,'teal');
  add('lock',s('M3 7h10v7H3ZM5 7V5a3 3 0 0 1 6 0v2')+r(7,9,2,3),'teal');
  add('unlock',s('M3 7h10v7H3ZM8 7V5a3 3 0 0 1 6 0')+r(7,9,2,3),'amber');
  add('priority-high',s('M3 8l5-5 5 5M3 13l5-5 5 5'),'red');
  add('priority-normal',r(3,5,10,2)+r(3,10,10,2),'blue');
  add('priority-low',s('M3 3l5 5 5-5M3 8l5 5 5-5'));
  add('skip',o(8,8,6)+s('M4 4l8 8'));
  add('folder',s('M2 4h4l2 2h6v7H2Z'),'amber');
  add('file-audio',p('M6 3l8-2v10h-2V5L8 6v7H6Z')+c(4,12,2)+c(11,10,2),'purple');
  add('file-video',frame(2,3,12,10)+p('M6 5l5 3-5 3Z'),'blue');
  add('file-image',frame(2,2,12,12)+c(5,5,1.25)+p('M3 12l4-5 3 3 2-2 2 4Z'),'teal');
  // A lidded crate rather than a narrow zipper/thermometer.
  add('file-archive',s('M3 6h10v8H3ZM2 3h12v3H2Z')+r(6,8,4,2),'amber');
  add('help',o(8,8,6)+s('M6 6a2 2 0 1 1 3 1l-1 1v1')+r(7,11,2,2),'blue');
  add('warning',`<path d="M8 1l7 13H1ZM7 5v4h2V5ZM7 11v2h2v-2Z" fill="currentColor" fill-rule="evenodd"/>`,'amber');
  add('close',s('M3 3l10 10M13 3 3 13'));
  add('menu',r(2,3,12,2)+r(2,7,12,2)+r(2,11,12,2));
  add('back',p('M7 2l-6 6 6 6v-4h8V6H7Z'));
  add('expand',s('M3 5l5 5 5-5'));
  add('collapse',s('M3 11l5-5 5 5'));
  add('select-all',frame(2,2,12,12)+s('M4 8l3 3 5-6'),'blue');
  add('clear-selection',frame(2,2,12,12));
  add('sort-ascending',p('M3 14V6H1l3-4 3 4H5v8Z')+r(9,2,6,2)+r(9,6,4,2)+r(9,10,2,2));
  add('sort-descending',p('M3 2h2v8h2l-3 4-3-4h2Z')+r(9,2,6,2)+r(9,6,4,2)+r(9,10,2,2));
  add('queueing',r(1,2,8,2)+r(1,6,8,2)+r(1,10,5,2)+p('M11 7h2v4h2l-3 4-3-4h2Z'),'blue');
  add('desktop-integration',s('M2 2h12v9H2ZM8 11v3M5 14h6'),'blue');
  add('authentication',o(5,6,3)+s('M8 6h7M13 6v3M10 6v2'),'amber');
  add('random-port',s('M2 2h12v12H2Z')+r(4,4,2,2)+r(10,4,2,2)+r(7,7,2,2)+r(4,10,2,2)+r(10,10,2,2),'blue');
  add('reload-ip-filter',H.px(0,0,['','...####','.########','.##....##','.##....##','.##....##','.##....##','..##..##','...####','....##'])+H.refreshBadge,'teal');
  add('more',r(2,7,2,2)+r(7,7,2,2)+r(12,7,2,2));
  // A table (header bar, three columns) above the shared refresh badge.
  add('reset-columns',H.px(0,1,['.#############','.#############','.#...#...#...#','.#...#...#...#','.#...#...#...#','.#############'])+H.refreshBadge,'neutral');
  add('logout',s('M7 2H2v12h5')+p('M10 3l5 5-5 5v-4H5V7h5Z'),'red');
  // A page carrying the app's P mark: "files of this type open in PaTorrent".
  add('association',H.px(0,1,['..########','..#########','..##......##','..##.####..##','..##.##.##.##','..##.##.##.##','..##.####..##','..##.##....##','..##.##....##','..##.##....##','..##.......##','..###########','..###########']),'teal');
  add('tray',s('M2 10v4h12v-4')+p('M7 1h2v6h3l-4 4-4-4h3Z'),'teal');
  return result;
};
