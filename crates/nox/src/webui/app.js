// PaTorrent's embedded Web UI: a torrent list kept current with sync/maindata.
"use strict";
(function () {
  const $ = (id) => document.getElementById(id);
  const torrents = new Map();
  let rid = 0;
  let timer = 0;

  function size(n) {
    if (n < 0) return "";
    const u = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return (i ? n.toFixed(1) : n) + " " + u[i];
  }
  const speed = (n) => (n > 0 ? size(n) + "/s" : "");
  function eta(s) {
    if (s >= 8640000) return "∞";
    if (s <= 0) return "";
    const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60;
    return d ? d + "d " + h + "h" : h ? h + "h " + m + "m" : m + "m " + (s % 60) + "s";
  }

  async function api(path, form) {
    const opts = { method: form ? "POST" : "GET", credentials: "same-origin" };
    if (form) opts.body = new URLSearchParams(form);
    const r = await fetch("api/v2/" + path, opts);
    if (r.status === 403) { showLogin(); throw new Error("forbidden"); }
    if (!r.ok) throw new Error(path + ": HTTP " + r.status);
    return r;
  }

  function cell(tr, text, cls) {
    const td = document.createElement("td");
    if (cls) td.className = cls;
    td.textContent = text;
    tr.appendChild(td);
    return td;
  }

  function button(label, onclick) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    b.onclick = onclick;
    return b;
  }

  function render() {
    const body = $("rows");
    body.textContent = "";
    const list = [...torrents.entries()].sort((a, b) => (a[1].name || "").localeCompare(b[1].name || ""));
    for (const [hash, t] of list) {
      const tr = document.createElement("tr");
      const name = cell(tr, t.name || hash, "name");
      name.title = t.name || hash;
      cell(tr, size(t.size), "num");
      const done = cell(tr, "", "num");
      const bar = document.createElement("span");
      bar.className = "bar";
      const fill = document.createElement("i");
      fill.style.width = ((t.progress || 0) * 100).toFixed(1) + "%";
      bar.appendChild(fill);
      done.appendChild(bar);
      done.append(" " + ((t.progress || 0) * 100).toFixed(1) + "%");
      cell(tr, t.state || "");
      cell(tr, speed(t.dlspeed), "num");
      cell(tr, speed(t.upspeed), "num");
      cell(tr, String(t.num_seeds ?? ""), "num");
      cell(tr, String(t.num_leechs ?? ""), "num");
      cell(tr, eta(t.eta), "num");
      const actions = cell(tr, "");
      const stopped = /^paused/.test(t.state || "");
      actions.appendChild(button(stopped ? "Start" : "Stop", () => api(stopped ? "torrents/start" : "torrents/stop", { hashes: hash }).then(poll)));
      actions.append(" ");
      actions.appendChild(button("Remove", () => {
        if (confirm("Remove " + (t.name || hash) + "? The downloaded files are kept.")) api("torrents/delete", { hashes: hash, deleteFiles: "false" }).then(poll);
      }));
      body.appendChild(tr);
    }
  }

  async function poll() {
    clearTimeout(timer);
    try {
      const d = await (await api("sync/maindata?rid=" + rid)).json();
      if (d.full_update) torrents.clear();
      rid = d.rid;
      for (const [hash, t] of Object.entries(d.torrents || {})) torrents.set(hash, Object.assign(torrents.get(hash) || {}, t));
      for (const hash of d.torrents_removed || []) torrents.delete(hash);
      const s = d.server_state;
      if (s) $("status").dataset.text = "↓ " + (speed(s.dl_info_speed) || "0 B/s") + "  ↑ " + (speed(s.up_info_speed) || "0 B/s") + "  DHT " + s.dht_nodes;
      $("status").textContent = $("status").dataset.text || "";
      render();
      timer = setTimeout(poll, 1500);
    } catch (e) {
      if (e.message !== "forbidden") timer = setTimeout(poll, 5000);
    }
  }

  function showLogin() {
    clearTimeout(timer);
    $("app").hidden = true;
    $("login").hidden = false;
  }

  function showApp() {
    $("login").hidden = true;
    $("app").hidden = false;
    rid = 0;
    poll();
  }

  $("login").onsubmit = async (ev) => {
    ev.preventDefault();
    const f = new FormData(ev.target);
    const r = await fetch("api/v2/auth/login", { method: "POST", body: new URLSearchParams(f), credentials: "same-origin" });
    const text = await r.text();
    if (r.ok && text === "Ok.") { $("login-error").textContent = ""; showApp(); }
    else $("login-error").textContent = r.status === 403 ? text : "Wrong user name or password.";
  };
  $("logout").onclick = () => api("auth/logout", {}).then(showLogin, showLogin);
  $("add").onsubmit = (ev) => {
    ev.preventDefault();
    const input = ev.target.elements.urls;
    if (!input.value.trim()) return;
    api("torrents/add", { urls: input.value.trim() }).then(() => { input.value = ""; poll(); });
  };

  api("app/version").then(showApp, () => {});
})();
