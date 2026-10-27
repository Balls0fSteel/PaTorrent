// Points the visitor at their own system's files: the home page's hidden direct-download line
// for it, and a "Your system" badge on the Download page. Without JavaScript the pages work
// as they are; nothing is stored or sent.
(function () {
  "use strict";
  var ua = navigator.userAgent || "";
  var os = null;
  if (/Windows NT/.test(ua)) {
    os = "windows";
  } else if (/Macintosh|Mac OS X/.test(ua) && !(navigator.maxTouchPoints > 1)) {
    os = "macos"; // an iPad asking for the desktop site also says Macintosh
  } else if (/Linux|X11/.test(ua) && !/Android|CrOS/.test(ua)) {
    os = "linux";
  }
  if (!os) return;
  var els = document.querySelectorAll('[data-os="' + os + '"]');
  for (var i = 0; i < els.length; i++) els[i].hidden = false;
})();
