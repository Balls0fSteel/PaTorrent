// Complete semantic inventory and independently editable small/large drawings.
'use strict';
const H=require('./common.cjs'),inventory=require('../inventory.json');
const modules=['states','actions','misc'].map(name=>require('./'+name+'.cjs')(H));
const geometry={};
for(const part of modules)for(const [id,glyph] of Object.entries(part)){
 if(geometry[id])throw Error('Duplicate glyph definition: '+id);
 geometry[id]=glyph;
}
// A flat symbolic rendition of the selected Verdigris: copper medallion + P/down-arrow.
// Transparent interior preserves the mark in monochrome and high-contrast use.
geometry.about={color:'teal',small:'<circle cx="8" cy="8" r="6" fill="none" stroke="__COPPER__" stroke-width="2"/>'+H.p('M5 3h4a3 3 0 0 1 0 6H7v1h2l-3 3-3-3h2Zm2 2v2h2a1 1 0 0 0 0-2Z').replace('fill="currentColor"','fill="currentColor" fill-rule="evenodd"'),large:'<circle cx="8" cy="8" r="6.5" fill="none" stroke="__COPPER__" stroke-width="1.5"/>'+H.p('M5 3h4a3 3 0 0 1 0 6H7v1h2l-3 3-3-3h2Zm2 1.5v3h2a1.5 1.5 0 0 0 0-3Z').replace('fill="currentColor"','fill="currentColor" fill-rule="evenodd"')};
const icons=inventory.map(meta=>{
 const glyph=geometry[meta.id];
 if(!glyph||!glyph.small)throw Error('Missing drawing: '+meta.id);
 return {...meta,...glyph,revised:true};
});
for(const id of Object.keys(geometry))if(!inventory.some(i=>i.id===id))throw Error('Unknown drawing: '+id);
const reuse=[['upload','seeding'],['download','downloading'],['pause','paused-incomplete'],['stop','stopped-incomplete'],['recheck','checking-files'],['filter','rss-filters'],['reannounce','tracker-updating']];
const runtime=['downloading','force-downloading','seeding','force-seeding','downloading-metadata','force-downloading-metadata','checking-files','checking-resume-data','queued-checking','queued-resume-checking','queued-download','queued-seed','paused-incomplete','paused-complete','stopped-incomplete','stopped-complete','moving','error','tracker-not-contacted','tracker-working','tracker-updating','tracker-error','network-ok','network-no-incoming'];
const toolbar=['add-torrent','add-url','create-torrent','remove','start','pause','stop','queue-up','queue-down','rss','preferences'];
module.exports={icons,reuse,runtime,toolbar};
