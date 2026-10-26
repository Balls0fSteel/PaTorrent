'use strict';

// Full action/RSS inventory. Badges occupy space beside intact base drawings.
module.exports = H => {
 const {r,p,s,c,o}=H;
 const even=d=>`<path d="${d}" fill="currentColor" fill-rule="evenodd"/>`;
 const page=even('M1 1h4l3 3v7H1Z M3 3v6h3V5H4V3Z');
 const pageLarge=even('M1 1h4l3 3v7H1Z M2.5 2.5v7h4V4.5H4V2.5Z');
 const plus=r(11,9,2,6)+r(9,11,6,2);
 const tag=even('M1 1h5l4 4-5 5-4-4Z M3 3v2h2V3Z');
 const trash=even('M4 6h8v9H4Z M6 8v5h1V8Z M9 8v5h1V8Z')+r(3,3,10,2)+r(6,1,4,2);
 const miniTrash=even('M2 6h5v8H2Z M4 8v4h1V8Z')+r(1,3,7,2)+r(3,1,3,2);
 const list=r(1,2,5,2)+r(1,7,5,2)+r(1,12,5,2);
 const upChevron=s('M9 9l2.5-3 2.5 3');
 const downChevron=s('M9 7l2.5 3 2.5-3');
 const folder=even('M1 1h4l2 2h3v5H1Z M3 5v1h5V5Z');
 const rss=c(2,8,1)+s('M2 5a3 3 0 0 1 3 3M2 2a6 6 0 0 1 6 6');
 const rssLarge=c(2,8,1)+s('M2 5a3 3 0 0 1 3 3M2 2a6 6 0 0 1 6 6',1.5);
 const bolt=p('M13 1l-3 7h2l-1 7 4-9h-3Z');
 const gauge=H.px(0,0,['','.....######','...##......##','..#..........#','.#........##..#','.#.......##...#','.#......##....#','.#.....##.....#','.##############']);
 return {
  'add-torrent':{small:page+plus,large:pageLarge+plus,color:'teal'},
  'add-url':{
   // A chain link (D29): two open loops joined by a bar. Magnet links are URLs too.
   small:H.px(0,0,['','..#####..#####','.##..........##','.#...######...#','.#...######...#','.##..........##','..#####..#####'])+plus,color:'teal'},
  'create-torrent':{small:page+p('M9 12l4-4 2 2-4 4-3 1Z'),large:pageLarge+p('M9 12l4-4 2 2-4 4-3 1Z'),color:'teal'},
  remove:{small:trash,color:'red'},
  'delete-data':{small:miniTrash+s('M10 10l4 4m0-4-4 4'),large:miniTrash+s('M10 10l4 4m0-4-4 4'),color:'red'},
  start:{small:p('M3 2l11 6-11 6Z'),color:'teal'},
  pause:{small:H.pause,color:'amber'},
  stop:{small:H.stop,color:'neutral'},
  'force-start':{small:p('M1 3l7 5-7 5Z')+bolt,large:p('M1 3l7 5-7 5Z')+bolt,color:'teal'},
  'queue-up':{small:list+upChevron,color:'blue'},
  'queue-down':{small:list+downChevron,color:'blue'},
  'queue-top':{small:list+upChevron+r(8,2,7,2),color:'blue'},
  'queue-bottom':{small:list+downChevron+r(8,12,7,2),color:'blue'},
  recheck:{small:H.verify,color:'amber'},
  reannounce:{small:H.reannounce,color:'blue'},
  'open-folder':{small:s('M2 10V3h4l2 2h5v2')+even('M4 7h11l-3 7H1Z M5 9l-1 3h6l1-3Z'),large:s('M2 10V3h4l2 2h5v2',1.5)+even('M4 7h11l-3 7H1Z M5 9l-1 3h6l1-3Z'),color:'amber'},
  'set-location':{small:folder+p('M8 11h4V9l3 3-3 3v-2H8Z'),large:folder+p('M8 11h4V9l3 3-3 3v-2H8Z'),color:'blue'},
  properties:{small:even('M2 1h8l3 3v11H2Z M4 3v10h7V5H9V3Z')+r(5,6,5,1)+r(5,10,5,1)+r(6,5,1,3)+r(8,9,1,3),large:even('M2 1h8l3 3v11H2Z M3.5 2.5v11h8v-9h-2v-2Z')+r(5,6,5,1)+r(5,10,5,1)+r(6,5,1,3)+r(8,9,1,3),color:'blue'},
  copy:{small:p('M1 11V1h10v2H3v8Z')+even('M5 5h10v10H5Z M7 7v6h6V7Z'),color:'blue'},
  save:{small:even('M1 1h11l3 3v11H1Z M4 1v5h6V1Z M4 9v4h8V9Z')+r(8,2,1,3),large:even('M1 1h11l3 3v11H1Z M4 1v5h6V1Z M4 9v4h8V9Z')+r(8,2,1,3)+r(6,10,4,1),color:'blue'},
  refresh:{small:s('M13 5a5 5 0 1 0 0 6')+p('M10 1h5v5Z'),color:'teal'},
  // A speedometer (flat-based half dial, needle) with a small down/up arrow badge.
  'download-limit':{small:gauge+H.px(9,10,['..##..','..##..','######','.####.','..##..']),color:'teal'},
  'upload-limit':{small:gauge+H.px(9,10,['..##..','.####.','######','..##..','..##..']),color:'blue'},
  unlimited:{small:s('M8 8C5 2 2 3 2 8s3 6 6 0c3-6 6-5 6 0s-3 6-6 0'),color:'blue'},
  'initial-seeding':{small:p('M7 10V7H3l5-5 5 5H9v3Z')+c(3,13,1)+c(8,13,1)+c(13,13,1),large:p('M7 10V7H3l5-5 5 5H9v3Z')+c(3,13,1)+c(8,13,1)+c(13,13,1),color:'blue'},
  'new-label':{small:tag+plus,large:tag+plus,color:'purple'},
  'remove-label':{small:tag+r(9,12,6,2),large:tag+r(9,12,6,2),color:'red'},
  'add-feed':{small:rss+plus,large:rssLarge+plus,color:'orange'},
  'rss-items':{small:rss+r(10,2,5,2)+r(10,6,5,2)+r(1,12,14,2),large:rssLarge+r(10,2,5,1.5)+r(10,6,5,1.5)+r(1,12,14,1.5),color:'orange'},
  'rss-history':{small:rss+o(11.5,11.5,2.5,1)+s('M11.5 9.5v2h2',1),large:rssLarge+o(11.5,11.5,2.5,1)+s('M11.5 9.5v2h2',1),color:'orange'},
  'rss-filters':{small:H.filter,color:'orange'}
 };
};
