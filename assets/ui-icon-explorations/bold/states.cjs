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
  // Revision 7: the four checking states share the checking lens (H.lens). Checking
  // files shows the tick in the hole; checking resume data shows "..." (a quick, pending
  // look rather than a verified result). The queued pair is the list with the matching
  // 6 x 6 lens badge or the "..." badge, so the floppy and the 1 px clock are gone.
  const dotsInLens = H.px(4,6,['#.#.#','#.#.#']);
  const miniLens = H.px(9,9,['.###.','#...#','#...#','#...#','.####','.....#']);
  const metadata = even('M1 1h4l3 3v8H1Z M3 3v7h3V5H4V3Z');
  const metadataLarge = even('M1 1h4l3 3v8H1Z M2.5 2.5v8h4V4.5h-2V2.5Z')+r(3.5,6,2,1)+r(3.5,8,2,1);
  const metadataDown = p('M11 8h2v3h2l-3 4-3-4h2Z');
  const metadataBolt = p('M12 1l-3 4h3l-2 2 5-4h-3l2-2Z');
  // Revision 6 (D30): a globe (outline, equator, lens meridian). The arrow-into-port
  // base read as "?" (open ring) and then as an eye or key (closed ring) at 16 px.
  const globe = H.px(0,0,[
    '',
    '....####',
    '..##.##.##',
    '.#..#..#..#',
    '.#..#..#..#',
    '.##########',
    '.#..#..#..#',
    '.#..#..#..#',
    '..##.##.##',
    '....####']);
  // 5 x 5 pixel badges in the bottom-right corner, each clear of its base by 1 px.
  const okBadge = H.px(10,10,['....#','...##','#.##.','###..','.#...']);
  const bangBadge = H.px(12,10,['##','##','##','..','##']);
  const crossBadge = H.px(10,10,['##.##','.###.','..#..','.###.','##.##']);
  const moreBadge = H.px(10,13,['#.#.#','#.#.#']);
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
    'checking-resume-data': {small:H.lens+dotsInLens, color:'amber'},
    'queued-checking': {small:list+miniLens, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+miniLens, color:'amber'},
    'queued-resume-checking': {small:list+moreBadge, large:r(1,2,6,1.5)+r(1,6,6,1.5)+r(1,10,6,1.5)+moreBadge, color:'amber'},
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
    // A drive outline filled to about half its height: space being reserved.
    allocating: {
      small:H.px(0,0,[
        '................',
        '................',
        '................',
        '.##############',
        '.##############',
        '.##..........##',
        '.##..........##',
        '.##..........##',
        '.##.########.##',
        '.##.########.##',
        '.##.########.##',
        '.##############',
        '.##############']),
      color:'amber'
    },
    'wanted-complete': {small:folder+badgeCheck, large:folder+badgeCheck, color:'teal'},
    // One tower base for all four tracker states; only the badge changes.
    'tracker-working': {small:H.tower+okBadge, color:'teal'},
    'tracker-updating': {small:H.reannounce, color:'blue'},
    'tracker-error': {small:H.tower+crossBadge, color:'red'},
    'tracker-not-contacted': {small:H.tower+moreBadge, color:'neutral'},
    'network-ok': {small:globe+okBadge, color:'teal'},
    // "No incoming connection seen yet" is a caution, not a fault: same base, "!".
    'network-no-incoming': {small:globe+bangBadge, color:'amber'},
    offline: {small:globe+crossBadge, color:'neutral'}
  };
};
