'use strict';

// Revision 3: compact state symbols are designed directly on the 16 px grid.
// Each compound keeps its entire base beside its modifier; no badge cuts a
// folder, document, server, arrow or pause bar apart.
module.exports = (H) => {
  const {r,p,s,c,o} = H;
  const even = d => `<path d="${d}" fill="currentColor" fill-rule="evenodd"/>`;
  const badgeCheck = p('M9 11l2 2 3-4 1 1-4 5-3-3Z');
  const bolt = p('M13 1l-3 7h2l-1 7 4-9h-3Z');
  const attention = r(12,2,2,7)+r(12,12,2,2);
  const leftDown = p('M4 2h2v7h3l-4 5-4-5h3Z');
  const leftUp = p('M4 14h2V7h3L5 2 1 7h3Z');
  const list = r(1,2,6,2)+r(1,6,6,2)+r(1,10,6,2);
  const smallDown = p('M11 7h2v4h2l-3 4-3-4h2Z');
  const smallUp = p('M11 15h2v-4h2l-3-4-3 4h2Z');
  const miniVerify = o(11,10,2)+s('M12 12l2 2');
  const miniClock = o(11.5,11.5,2.5,1)+s('M11.5 9.5v2h2',1);
  const server = even('M1 2H8V12H1Z M3 4v1h3V4Z M3 7v1h3V7Z M3 10v1h3v-1Z');
  const metadata = even('M1 1h4l3 3v8H1Z M3 3v7h3V5H4V3Z');
  const metadataLarge = even('M1 1h4l3 3v8H1Z M2.5 2.5v8h4V4.5h-2V2.5Z')+r(3.5,6,2,1)+r(3.5,8,2,1);
  const metadataDown = p('M11 8h2v3h2l-3 4-3-4h2Z');
  const metadataBolt = p('M12 1l-3 4h3l-2 2 5-4h-3l2-2Z');
  const floppy = even('M1 2h6l1 1v8H1Z M3 2v3h3V2Z M3 8v1h3V8Z');
  const plug = even('M2 4h6v5l-2 2v2H4v-2L2 9Z M4 6v2h2V6Z')+r(2,1,2,3)+r(6,1,2,3)+r(4,13,2,2);
  const incoming = p('M10 10l2 2 2-3 1 1-3 5-4-4Z');
  const waiting = r(11,8,2,4)+r(11,14,2,1);
  const person = c(4,4,2)+p('M1 14V10a3 3 0 0 1 6 0v4Z');
  const folder = even('M1 1h4l2 2h3v5H1Z M3 5v1h5V5Z');
  const error = even('M5 1h6l4 4v6l-4 4H5l-4-4V5Z M5.8 3 3 5.8v4.4L5.8 13h4.4l2.8-2.8V5.8L10.2 3Z')+s('M5 5l6 6m0-6-6 6');

  return {
    downloading: {small:H.down, color:'teal'},
    seeding: {small:H.up, color:'blue'},
    'force-downloading': {small:leftDown+bolt, large:leftDown+p('M13 1l-3 6.5h2l-1 7.5 4-9h-3Z'), color:'teal'},
    'force-seeding': {small:leftUp+bolt, large:leftUp+p('M13 1l-3 6.5h2l-1 7.5 4-9h-3Z'), color:'blue'},
    'downloading-metadata': {small:metadata+metadataDown, large:metadataLarge+metadataDown, color:'teal'},
    'force-downloading-metadata': {
      // Same document and down arrow as metadata; lightning is separated
      // above the down arrow rather than squeezing a second cue inside it.
      small:metadata+metadataDown+metadataBolt,
      large:metadataLarge+metadataDown+metadataBolt,
      color:'teal'
    },
    'checking-files': {small:H.verify, color:'amber'},
    'checking-resume-data': {small:floppy+miniClock, large:floppy+miniClock, color:'amber'},
    'queued-checking': {small:list+miniVerify, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+miniVerify, color:'amber'},
    'queued-resume-checking': {small:list+miniClock, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+miniClock, color:'amber'},
    'queued-download': {small:list+smallDown, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+smallDown, color:'amber'},
    'queued-seed': {small:list+smallUp, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+smallUp, color:'amber'},
    'paused-incomplete': {small:H.pause, color:'amber'},
    'paused-complete': {small:r(1,2,2,11)+r(5,2,2,11)+badgeCheck, large:r(1,2,2,11)+r(5,2,2,11)+badgeCheck, color:'amber'},
    'stopped-incomplete': {small:H.stop, color:'neutral'},
    'stopped-complete': {small:r(1,3,7,7)+badgeCheck, large:r(1,3,7,7)+badgeCheck, color:'neutral'},
    moving: {
      small:p('M2 3h8V1l5 4-5 4V7H2Z')+p('M14 13H6v2l-5-4 5-4v2h8Z'),
      large:p('M2 3h8V1l5 4-5 4V7H2Z')+p('M14 13H6v2l-5-4 5-4v2h8Z'),
      color:'blue'
    },
    error: {small:error, color:'red'},
    'stalled-download': {small:leftDown+attention, color:'amber'},
    'stalled-seed': {small:leftUp+attention, color:'amber'},
    'no-peers': {small:person+s('M10 3l4 4m0-4-4 4'), large:person+s('M10 3l4 4m0-4-4 4'), color:'neutral'},
    allocating: {
      small:even('M2 3h12v10H2Z M4 5v6h8V5Z')+r(4,8,3,3)+r(10,8,2,2),
      large:even('M2 3h12v10H2Z M3.5 4.5v7h9v-7Z')+r(4,8,3,3)+r(9,8,1,2)+r(11,8,1,2),
      color:'amber'
    },
    'wanted-complete': {small:folder+badgeCheck, large:folder+badgeCheck, color:'teal'},
    'tracker-working': {small:H.check, color:'teal'},
    'tracker-updating': {small:H.announce, color:'blue'},
    'tracker-error': {small:server+s('M10 10l4 4m0-4-4 4'), large:server+s('M10 10l4 4m0-4-4 4'), color:'red'},
    'tracker-not-contacted': {small:even('M1 2h7v10H1Z M2 3v8h5V3Z')+o(12,12,2), large:even('M1 2h7v10H1Z M2.5 3.5v7h4V3.5Z')+o(12,12,2,1.5), color:'neutral'},
    'network-ok': {small:plug+incoming, large:plug+incoming, color:'teal'},
    'network-no-incoming': {small:plug+waiting, large:plug+r(11,8,2,4)+r(11,14,2,1), color:'amber'},
    offline: {
      // Same intact plug as the observed-connectivity states, with a clear cross.
      small:plug+s('M10 8l4 4m0-4-4 4'),
      large:plug+s('M10 8l4 4m0-4-4 4'),
      color:'neutral'
    }
  };
};
