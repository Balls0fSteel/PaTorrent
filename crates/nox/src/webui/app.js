// PaTorrent's Web UI: the µTorrent 2.2.1 main window on the native session protocol
// (docs/WEBUI.md). The page holds only the visible window of rows; the server runs the
// filtered, sorted list and sends formatted cells, and the detail pane's tab content the
// same way. Commands are POSTs to the native endpoints (pa/...) with JSON bodies; the compat
// shim (api/v2/...) is used only for the login and for reads the native API has no need of
// yet (the context menu's check marks).
"use strict";
(() => {
  const ROW_H = 22; // must match --row-h in style.css (set below so they cannot drift)
  const PHONE_ROW_H = 60; // a phone row's height, --phone-row-h (set below too)
  // the phone list's cells: Name over the progress bar, then status, speeds and ETA
  const PHONE_COLS = ["name", "done", "status", "down_speed", "up_speed", "eta"];
  const PHONE = matchMedia("(max-width: 767px)"); // the same query as style.css
  const LONG_PRESS_MS = 450;
  const OVERSCAN = 10;
  const MAX_WINDOW = 200;
  const LIMITS = [0, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000]; // kB/s, 0 = unlimited
  // frontend::detail::PRIORITIES: the menu's order (string table keys) and the engine's values
  const PRIORITIES = [
    ["PrioHighItem", 7],
    ["PrioNormalItem", 4],
    ["PrioLowItem", 1],
    ["PrioDontDownload", 0],
  ];
  const GATE = location.hash === "#gate";
  const $ = (id) => document.getElementById(id);

  // ---- the string table (lang.js: frontend::lang in the server's language) -----------------
  // T(key) is a text, T(key, {name: value}) fills its {name} placeholders. Texts the server
  // sends (cells, columns, the General tab, Preferences) come translated already.
  const LANG = (window.PA_LANG && window.PA_LANG.t) || {};
  // an Arabic, Hebrew or Persian table mirrors the page (dir="rtl"): drags and menus follow
  const RTL = !!(window.PA_LANG && window.PA_LANG.dir === "rtl");
  function T(key, args) {
    const s = LANG[key] === undefined ? key : LANG[key]; // an unknown key shows itself: a bug
    return args ? s.replace(/\{([a-z0-9_]+)\}/g, (m, n) => (n in args ? String(args[n]) : m)) : s;
  }
  /// The static page's texts: data-t is an element's text, data-t-title, data-t-aria and
  /// data-t-ph its title, aria-label and placeholder.
  function translatePage() {
    if (window.PA_LANG) document.documentElement.lang = window.PA_LANG.code;
    document.documentElement.dir = RTL ? "rtl" : "ltr";
    for (const el of document.querySelectorAll("[data-t]")) el.textContent = T(el.dataset.t);
    for (const el of document.querySelectorAll("[data-t-title]")) el.title = T(el.dataset.tTitle);
    for (const el of document.querySelectorAll("[data-t-aria]")) el.setAttribute("aria-label", T(el.dataset.tAria));
    for (const el of document.querySelectorAll("[data-t-ph]")) el.placeholder = T(el.dataset.tPh);
    $("tb-prefs").title = T("ToolPreferences") + " (" + T("KeyCtrl") + "+P)";
  }
  translatePage();

  const store = {
    get(key, fallback) {
      try {
        const v = localStorage.getItem(key);
        return v === null ? fallback : JSON.parse(v);
      } catch (e) {
        return fallback;
      }
    },
    set(key, value) {
      try {
        localStorage.setItem(key, JSON.stringify(value));
      } catch (e) {
        /* private mode or full: the layout just does not persist */
      }
    },
  };

  // ---- state ---------------------------------------------------------------------------
  let ws = null;
  let closing = false; // a deliberate close (logout): no reconnect
  let backoff = 1000;
  let reconnectTimer = 0;
  let countdownTimer = 0;
  let hello = null; // {version, columns, categories, labels}
  const colById = new Map();
  let layout = store.get("pa.columns", { order: [], widths: {}, hidden: null });
  let sort = store.get("pa.sort", ["name", false]);
  let filter = store.get("pa.filter", { category: "all", label: null });
  let search = "";
  let total = 0;
  let winFirst = 0;
  let ids = []; // the window's ids, in view order
  let cells = new Map(); // id -> {v, c, done, state}
  let sentWindow = null; // [first, count] of the last view sent
  let colsGen = 0; // bumped when the visible columns change, so rows refill
  let sel = new Set();
  let selAll = false;
  let anchor = -1; // selection anchor, an index in the view
  let cur = -1; // the focused row, an index in the view
  let pendingCur = -1; // a focus moved outside the window, applied when its row arrives
  let detailTab = store.get("pa.tab", "general");
  let detailId = null;
  let labels = [];
  let labelCounts = new Map(); // label ("" for none) to its torrent count
  let pool = [];
  let scrollRaf = 0;
  let hidden = document.hidden;
  let phone = PHONE.matches;
  let rowH = phone ? PHONE_ROW_H : ROW_H;
  let selecting = false; // the phone's selection mode: a tap toggles a row
  let sheetOpen = false; // the phone's detail sheet

  const list = $("list");
  const body = $("body");
  const spacer = $("spacer");
  const hdrRow = $("hdr-row");

  // ---- the gate panel (#gate): what the page itself measures ----------------------------
  const gate = {
    tStart: performance.now(),
    tLogin: 0,
    tPaint: 0,
    rowsBytes: 0,
    ticks: 0,
    tickBytes: 0,
    tickMin: Infinity,
    tickMax: 0,
    tickChanged: 0,
    hiddenMsgs: 0,
    hiddenSince: 0,
    hiddenSecs: 0,
    msgs: 0,
    stages: [], // [name, ms since login or load]
  };
  function stage(name) {
    if (GATE) gate.stages.push([name, performance.now() - (gate.tLogin || gate.tStart)]);
  }
  function gateMsg(data, m) {
    gate.msgs++;
    if (hidden) gate.hiddenMsgs++;
    if (!GATE) return;
    const n = new TextEncoder().encode(data).length;
    if (m.t === "rows" && !gate.rowsBytes) gate.rowsBytes = n;
    if (m.t === "tick") {
      gate.ticks++;
      gate.tickBytes = n;
      if (m.cells || m.ids) gate.tickChanged++;
      else {
        gate.tickMin = Math.min(gate.tickMin, n);
        gate.tickMax = Math.max(gate.tickMax, n);
      }
    }
    gateRender();
  }
  function gateRender() {
    if (!GATE) return;
    const el = $("gate");
    el.hidden = false;
    const since = gate.tPaint ? (gate.tPaint - (gate.tLogin || gate.tStart)).toFixed(0) + " ms" : "…";
    el.textContent =
      `${gate.tLogin ? "login" : "load"} to first painted list: ${since}\n` +
      `first rows: ${gate.rowsBytes} B (${ids.length} rows, ${viewColumns().length} cols)\n` +
      `ticks: ${gate.ticks}, last ${gate.tickBytes} B, idle min/max ${gate.tickMin === Infinity ? "-" : gate.tickMin}/${gate.tickMax} B, with changes ${gate.tickChanged}\n` +
      `messages while hidden: ${gate.hiddenMsgs} over ${gate.hiddenSecs.toFixed(0)} s hidden\n` +
      `total ${total}, window [${winFirst}, ${ids.length}]
` +
      `stages: ${gate.stages.map(([n, ms]) => n + " " + ms.toFixed(0)).join(", ")}`;
    // the window title too, so a script can read the numbers off a browser it cannot see into
    document.title =
      `PaTorrent gate | paint ${since} | rows ${gate.rowsBytes} B/${ids.length} | tick ${gate.tickBytes} B x${gate.ticks} ` +
      `(changed ${gate.tickChanged}) | hidden ${gate.hiddenMsgs} msgs/${gate.hiddenSecs.toFixed(0)} s | total ${total}`;
  }

  // ---- the native commands (docs/WEBUI.md "Commands") --------------------------------------
  // A JSON POST with the CSRF header; 204 on success, else {"error"} shown in the status bar.
  // `body` may be a FormData (the uploads) instead of an object.
  async function pa(path, body) {
    const opts = { method: body === undefined ? "GET" : "POST", credentials: "same-origin", headers: { "X-Requested-With": "PaTorrent" } };
    if (body instanceof FormData) opts.body = body;
    else if (body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = JSON.stringify(body);
    }
    const r = await fetch("pa/" + path, opts);
    if (r.status === 403) {
      let msg = "";
      try {
        msg = (await r.json()).error || "";
      } catch (e) {
        /* no body */
      }
      if (msg === "forbidden") {
        showLogin();
        throw new Error("forbidden");
      }
      flash(path + ": " + (msg || "forbidden"));
      throw new Error(path + ": " + msg);
    }
    if (!r.ok) {
      let msg = "";
      try {
        msg = (await r.json()).error || "";
      } catch (e) {
        /* no body */
      }
      flash(path + ": " + (msg || "HTTP " + r.status));
      const err = new Error(path + ": HTTP " + r.status);
      err.msg = msg; // the server's {"error"} text, for a dialog that shows it in place
      err.status = r.status;
      throw err;
    }
    return r.status === 204 ? null : r.json();
  }

  // ---- shim calls (login, and reads) ------------------------------------------------------
  async function api(path, form, raw) {
    const opts = { method: form ? "POST" : "GET", credentials: "same-origin" };
    if (form) opts.body = form instanceof FormData ? form : new URLSearchParams(form);
    const r = await fetch("api/v2/" + path, opts);
    if (r.status === 403) {
      showLogin();
      throw new Error("forbidden");
    }
    if (!r.ok) {
      const text = await r.text();
      flash(path + ": " + (text || "HTTP " + r.status));
      throw new Error(path + ": HTTP " + r.status);
    }
    return raw ? r : r.text();
  }

  let flashTimer = 0;
  function flash(text) {
    const el = $("st-conn");
    el.textContent = text;
    el.classList.add("bad");
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => {
      if (el.textContent === text) el.textContent = ws && ws.readyState === 1 ? "" : el.textContent;
      el.classList.remove("bad");
    }, 4000);
  }

  // ---- columns -------------------------------------------------------------------------
  function visibleColumns() {
    if (!hello) return [];
    const hiddenSet = new Set(layout.hidden || []);
    return layout.order.filter((id) => colById.has(id) && !hiddenSet.has(id));
  }
  /// The columns the view subscribes to: the header's on the desktop, a fixed set on a phone.
  function viewColumns() {
    if (!hello) return [];
    return phone ? PHONE_COLS.filter((id) => colById.has(id)) : visibleColumns();
  }
  function widthOf(id) {
    const w = layout.widths[id];
    return typeof w === "number" && w >= 20 ? w : colById.get(id).width;
  }
  function saveLayout() {
    store.set("pa.columns", layout);
  }
  function mergeColumns(columns) {
    colById.clear();
    for (const c of columns) colById.set(c.id, c);
    // the saved order first (dropping ids the server no longer knows), then new columns
    const order = (layout.order || []).filter((id) => colById.has(id));
    for (const c of columns) if (!order.includes(c.id)) order.push(c.id);
    layout.order = order;
    if (!layout.widths) layout.widths = {};
    if (!Array.isArray(layout.hidden)) layout.hidden = columns.filter((c) => !c.visible).map((c) => c.id);
    if (!colById.has(sort && sort[0])) sort = ["name", false];
  }

  /// Sets the grid template, the header cells and the pool rows' cells for the visible
  /// columns; `resubscribe` says whether the column list changed (widths alone do not).
  function applyColumns(resubscribe) {
    const cols = visibleColumns();
    const widths = cols.map(widthOf);
    const sum = widths.reduce((a, b) => a + b, 0);
    list.style.setProperty("--cols", widths.map((w) => w + "px").join(" "));
    list.style.setProperty("--cols-w", sum + "px");
    // header
    hdrRow.textContent = "";
    for (const id of cols) {
      const c = colById.get(id);
      const h = document.createElement("div");
      h.className = "hc" + (c.align === "right" ? " r" : "");
      h.dataset.col = id;
      h.textContent = c.title;
      h.title = c.title;
      if (sort && sort[0] === id) h.classList.add("sort", sort[1] ? "desc" : "asc");
      const rs = document.createElement("div");
      rs.className = "rs";
      h.appendChild(rs);
      hdrRow.appendChild(h);
    }
    if (resubscribe) {
      colsGen++;
      for (const el of pool) buildCells(el, viewColumns());
      sentWindow = null;
      sendView();
    }
    gateRender();
  }
  function buildCells(row, cols) {
    row.textContent = "";
    row.classList.toggle("prow", phone);
    row._gen = colsGen;
    row._id = null;
    if (phone) {
      // Name / the bar with the percentage / status, speeds and ETA; the actions button
      const name = document.createElement("div");
      name.className = "pn";
      const bar = document.createElement("div");
      bar.className = "bar";
      bar.append(document.createElement("i"), document.createElement("span"));
      const info = document.createElement("div");
      info.className = "pi";
      const mb = document.createElement("button");
      mb.type = "button";
      mb.className = "mb";
      mb.tabIndex = -1;
      mb.setAttribute("aria-label", T("WebActions"));
      mb.textContent = "⋯";
      row.append(name, bar, info, mb);
      return;
    }
    for (const id of cols) {
      const c = colById.get(id);
      const cell = document.createElement("div");
      cell.className = "cell" + (c.align === "right" ? " r" : "");
      if (id === "done") {
        cell.classList.add("done");
        const bar = document.createElement("div");
        bar.className = "bar";
        bar.appendChild(document.createElement("i"));
        cell.appendChild(bar);
        cell.appendChild(document.createElement("span"));
      }
      row.appendChild(cell);
    }
    row._id = null;
    row._gen = colsGen;
  }

  // ---- the WebSocket session -----------------------------------------------------------
  function wsUrl() {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    let path = location.pathname;
    if (!path.endsWith("/")) path = path.slice(0, path.lastIndexOf("/") + 1);
    return proto + "//" + location.host + path + "pa/ws";
  }
  function send(obj) {
    if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj));
  }
  function connect() {
    clearTimeout(reconnectTimer);
    clearInterval(countdownTimer);
    closing = false;
    setConn(T("WebConnecting"));
    const sock = new WebSocket(wsUrl());
    ws = sock;
    sock.onopen = () => stage("open");
    sock.onmessage = (ev) => {
      let m;
      try {
        m = JSON.parse(ev.data);
      } catch (e) {
        return;
      }
      gateMsg(ev.data, m);
      if (!gate.tPaint && (m.t === "hello" || m.t === "rows")) stage(m.t);
      switch (m.t) {
        case "hello":
          onHello(m);
          break;
        case "rows":
          onRows(m);
          break;
        case "tick":
          onTick(m);
          break;
        case "detail":
          onDetail(m);
          break;
        case "error":
          flash(m.msg || "error");
          break;
        default:
          break;
      }
    };
    sock.onclose = () => {
      if (ws !== sock) return;
      ws = null;
      if (closing) return;
      scheduleReconnect();
    };
    sock.onerror = () => {};
  }
  function scheduleReconnect() {
    const delay = backoff;
    backoff = Math.min(backoff * 2, 30000);
    let left = Math.ceil(delay / 1000);
    setConn(T("WebReconnecting", { secs: left }), true);
    clearInterval(countdownTimer);
    countdownTimer = setInterval(() => {
      left--;
      if (left > 0) setConn(T("WebReconnecting", { secs: left }), true);
    }, 1000);
    reconnectTimer = setTimeout(async () => {
      clearInterval(countdownTimer);
      // the session may have ended: a 403 means the login form, not another socket
      try {
        const r = await fetch("api/v2/app/version", { credentials: "same-origin" });
        if (r.status === 403) {
          showLogin();
          return;
        }
      } catch (e) {
        scheduleReconnect();
        return;
      }
      connect();
    }, delay);
  }
  function setConn(text, bad) {
    const el = $("st-conn");
    el.textContent = text;
    el.classList.toggle("bad", !!bad);
  }

  function onHello(m) {
    hello = m;
    // as µTorrent's window title did: the name and the version
    document.title = "PaTorrent " + m.version;
    backoff = 1000;
    setConn("");
    labels = m.labels || [];
    labelCounts = new Map();
    confirmDelete = m.confirm_delete !== false;
    // a desktop client hosts the Web UI: the RSS Downloader is the client's (no /pa/rss)
    $("tb-rss").hidden = !!m.hosted;
    mergeColumns(m.columns);
    saveLayout();
    renderTree();
    applyColumns(false);
    for (const el of pool) buildCells(el, viewColumns());
    sentWindow = null;
    if (hidden) send({ t: "idle" });
    else sendView();
    // a fresh socket: the server sends the tab whole again, into a fresh view
    clearDetail();
    if (detailId) send({ t: "detail", id: detailId, tab: detailTab });
  }

  /// The view subscription for what the list shows right now. `window` follows the scroll
  /// position; it is sent only when the visible rows are not covered by the last one.
  function sendView() {
    if (!hello) return;
    const [first, count] = neededWindow();
    sentWindow = [first, count];
    const f = { category: filter.label != null ? "all" : filter.category || "all", label: filter.label ?? null, search };
    send({ t: "view", filter: f, sort, columns: viewColumns(), window: [first, count] });
  }
  function neededWindow() {
    const visFirst = Math.floor(body.scrollTop / rowH);
    const visCount = Math.ceil(body.clientHeight / rowH) + 1;
    const first = Math.max(0, visFirst - OVERSCAN);
    const count = Math.min(MAX_WINDOW, visCount + 2 * OVERSCAN);
    return [first, count];
  }
  function windowCovers(first, count) {
    if (!sentWindow) return false;
    const [sf, sc] = sentWindow;
    const visFirst = Math.floor(body.scrollTop / rowH);
    const visLast = visFirst + Math.ceil(body.clientHeight / rowH);
    const margin = Math.floor(OVERSCAN / 2);
    const lo = sf === 0 ? 0 : sf + margin;
    const hi = sf + sc - margin;
    return visFirst >= lo && (visLast <= hi || sf + sc >= total) && first + count <= sf + sc + margin;
  }

  function onRows(m) {
    total = m.total;
    winFirst = m.first;
    ids = m.ids;
    cells = new Map();
    for (const id of ids) {
      const c = m.cells[id];
      if (c) cells.set(id, c);
    }
    afterRows();
  }
  /// A tick with nothing in the window changed costs a JSON parse and a few string compares;
  /// the rows are touched only when the order or a cell moved.
  function onTick(m) {
    const grew = m.total !== total;
    total = m.total;
    if (m.ids) {
      ids = m.ids;
      winFirst = m.first;
    }
    if (m.cells) for (const id in m.cells) cells.set(id, m.cells[id]);
    if (m.counts) updateCounts(m.counts);
    if (m.labels) updateLabels(m.labels, m.no_label);
    if (m.rss !== undefined) rssChanged(m.rss);
    if (m.adds !== undefined) addsChanged(m.adds);
    if (m.status) updateStatus(m.status);
    if (m.ids || m.cells) {
      // keep only the window's cells
      const keep = new Map();
      for (const id of ids) {
        const c = cells.get(id);
        if (c) keep.set(id, c);
      }
      cells = keep;
      afterRows();
    } else if (grew) {
      updateSpacer();
      queueScroll();
      gateRender();
    } else gateRender();
  }
  function afterRows() {
    updateSpacer();
    if (pendingCur >= 0 && pendingCur >= winFirst && pendingCur < winFirst + ids.length) {
      const i = pendingCur;
      pendingCur = -1;
      focusRow(i, pendingMode);
    }
    render();
    updateSelStatus();
    if (GATE && !gate.tPaint && ids.length) {
      requestAnimationFrame(() => {
        gate.tPaint = performance.now();
        stage("paint");
        console.info("PaTorrent: first painted list after", (gate.tPaint - (gate.tLogin || gate.tStart)).toFixed(0), "ms");
        gateRender();
      });
    }
    // the window may have been partly off: the rows landed, the scroll moved on
    queueScroll();
  }
  function updateSpacer() {
    spacer.style.height = total * rowH + "px";
    const empty = body.querySelector(".empty");
    if (total === 0 && hello) {
      if (!empty) {
        const e = document.createElement("div");
        e.className = "empty";
        e.textContent = search || filter.label != null || filter.category !== "all" ? T("WebNoMatch") : T("WebNoTorrents");
        body.appendChild(e);
      }
    } else if (empty) empty.remove();
  }

  // ---- the virtual list ----------------------------------------------------------------
  function render() {
    const cols = viewColumns();
    while (pool.length < ids.length) {
      const el = document.createElement("div");
      el.className = "row";
      el.setAttribute("role", "row");
      buildCells(el, cols);
      spacer.appendChild(el);
      pool.push(el);
    }
    for (let i = 0; i < pool.length; i++) {
      const el = pool[i];
      if (i >= ids.length) {
        if (!el.hidden) el.hidden = true;
        el._id = null;
        continue;
      }
      const id = ids[i];
      const index = winFirst + i;
      const c = cells.get(id);
      if (el.hidden) el.hidden = false;
      const top = index * rowH + "px";
      if (el._top !== top) {
        el.style.top = top;
        el._top = top;
      }
      if (el._gen !== colsGen) buildCells(el, cols);
      if (c && (el._id !== id || el._v !== c.v)) fillRow(el, id, c, cols);
      el._index = index;
      el.classList.toggle("sel", isSel(id));
      el.classList.toggle("cur", index === cur);
    }
  }
  function fillRow(el, id, c, cols) {
    el._id = id;
    el._v = c.v;
    el.dataset.id = id;
    if (el._state !== c.state) {
      if (el._state) el.classList.remove("st-" + el._state);
      el.classList.add("st-" + c.state);
      el._state = c.state;
    }
    if (phone) {
      const at = (id) => c.c[cols.indexOf(id)] || "";
      const [name, bar, info] = el.children;
      const set = (node, text) => {
        if (node.textContent !== text) node.textContent = text;
      };
      set(name, at("name"));
      name.title = at("name");
      bar.firstChild.style.width = c.done / 10 + "%";
      set(bar.lastChild, at("done"));
      const down = at("down_speed");
      const up = at("up_speed");
      const eta = at("eta");
      const parts = [at("status")];
      // speeds are empty at zero, like the desktop columns
      if (down) parts.push("↓ " + down);
      if (up) parts.push("↑ " + up);
      if (eta && eta !== "∞") parts.push(eta);
      set(info, parts.filter(Boolean).join(" · "));
      return;
    }
    for (let j = 0; j < cols.length; j++) {
      const cell = el.children[j];
      const text = c.c[j] || "";
      if (cols[j] === "done") {
        cell.firstChild.firstChild.style.width = c.done / 10 + "%";
        const span = cell.lastChild;
        if (span.textContent !== text) span.textContent = text;
      } else if (cell.textContent !== text) cell.textContent = text;
    }
  }
  function isSel(id) {
    return selAll || sel.has(id);
  }
  function idAt(index) {
    const i = index - winFirst;
    return i >= 0 && i < ids.length ? ids[i] : null;
  }
  function queueScroll() {
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0;
      $("hdr").scrollLeft = body.scrollLeft;
      const [first, count] = neededWindow();
      if (!windowCovers(first, count)) sendView();
    });
  }
  body.addEventListener("scroll", queueScroll, { passive: true });
  new ResizeObserver(() => queueScroll()).observe(body);

  // ---- selection and keyboard ----------------------------------------------------------
  let pendingMode = "set";
  function updateSelClasses() {
    for (const el of pool) {
      if (el.hidden) continue;
      el.classList.toggle("sel", isSel(el._id));
      el.classList.toggle("cur", el._index === cur);
    }
    updateSelStatus();
  }
  function updateSelStatus() {
    const n = selAll ? total : sel.size;
    $("st-sel").textContent = n ? T("WebSelected", { count: n }) : "";
    const none = n === 0;
    for (const b of document.querySelectorAll("#toolbar [data-cmd]")) b.disabled = none;
    if (selecting && none) selecting = false;
    document.body.classList.toggle("selecting", selecting);
    $("tb-seldone").hidden = !selecting;
  }
  function selectRange(a, b) {
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    for (let i = lo; i <= hi; i++) {
      const id = idAt(i);
      if (id) sel.add(id);
    }
  }
  /// Moves the focus to row `index` and applies the selection `mode`: "set" (that row
  /// alone), "add" (toggle it), "range" (anchor to it), "focus" (selection unchanged).
  function focusRow(index, mode) {
    if (total === 0) return;
    index = Math.max(0, Math.min(total - 1, index));
    const id = idAt(index);
    cur = index;
    if (!id) {
      pendingCur = index;
      pendingMode = mode;
      ensureVisible(index);
      updateSelClasses();
      return;
    }
    if (mode === "set") {
      sel = new Set([id]);
      selAll = false;
      anchor = index;
    } else if (mode === "add") {
      if (sel.has(id)) sel.delete(id);
      else sel.add(id);
      selAll = false;
      anchor = index;
    } else if (mode === "range") {
      if (anchor < 0) anchor = index;
      if (!selAll) {
        sel = new Set();
        selectRange(anchor, index);
      }
    }
    ensureVisible(index);
    updateSelClasses();
    // a phone subscribes to the detail only while its sheet is open
    if (!phone || sheetOpen) setDetail(id);
  }
  function ensureVisible(index) {
    const top = index * rowH;
    if (top < body.scrollTop) body.scrollTop = top;
    else if (top + rowH > body.scrollTop + body.clientHeight) body.scrollTop = top + rowH - body.clientHeight;
  }
  /// Subscribes the detail pane to the focused row (null for none: the pane stays empty).
  function setDetail(id) {
    if (id === detailId) return;
    detailId = id;
    clearDetail();
    send({ t: "detail", id, tab: detailTab });
  }
  function rowOf(target) {
    const el = target.closest(".row");
    return el && !el.hidden && el._id ? el : null;
  }
  body.addEventListener("pointerdown", (ev) => {
    lastPointer = ev.pointerType;
    if (phone) {
      phoneDown(ev);
      return;
    }
    if (ev.button !== 0 && ev.button !== 2) return;
    const el = rowOf(ev.target);
    if (!el) {
      if (ev.button === 0 && (ev.target === spacer || ev.target === body)) {
        sel = new Set();
        selAll = false;
        cur = -1;
        updateSelClasses();
        setDetail(null);
      }
      return;
    }
    if (ev.button === 2) {
      if (!isSel(el._id)) focusRow(el._index, "set");
      else {
        cur = el._index;
        updateSelClasses();
      }
      return;
    }
    const mode = ev.shiftKey ? "range" : ev.ctrlKey || ev.metaKey ? "add" : "set";
    focusRow(el._index, mode);
  });
  body.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    // a touch's long press is ours (selection mode), not the browser's context menu
    if (phone && lastPointer !== "mouse") return;
    const el = rowOf(ev.target);
    if (phone && el && !isSel(el._id)) focusRow(el._index, "set");
    if (!el && sel.size === 0 && !selAll) return;
    openTorrentMenu(ev.clientX, ev.clientY);
  });
  body.addEventListener("keydown", (ev) => {
    if (ev.target !== body) return;
    const page = Math.max(1, Math.floor(body.clientHeight / rowH) - 1);
    const mode = ev.shiftKey ? "range" : ev.ctrlKey || ev.metaKey ? "focus" : "set";
    const from = cur < 0 ? (ev.key === "ArrowUp" || ev.key === "End" || ev.key === "PageUp" ? total : -1) : cur;
    let handled = true;
    switch (ev.key) {
      case "ArrowDown":
        focusRow(from + 1, mode);
        break;
      case "ArrowUp":
        focusRow(from - 1, mode);
        break;
      case "PageDown":
        focusRow(from + page, mode);
        break;
      case "PageUp":
        focusRow(from - page, mode);
        break;
      case "Home":
        focusRow(0, mode);
        break;
      case "End":
        focusRow(total - 1, mode);
        break;
      case " ":
        if (ev.ctrlKey && cur >= 0) focusRow(cur, "add");
        else command("pause");
        break;
      case "Delete":
        command(ev.shiftKey ? "remove_data" : "remove");
        break;
      case "a":
      case "A":
        if (!(ev.ctrlKey || ev.metaKey)) handled = false;
        else {
          selAll = true;
          for (const id of ids) sel.add(id);
          updateSelClasses();
        }
        break;
      case "ContextMenu":
        menuAtCur();
        break;
      case "F10":
        if (ev.shiftKey) menuAtCur();
        else handled = false;
        break;
      case "Escape":
        closeMenu();
        break;
      default:
        handled = false;
    }
    if (handled) ev.preventDefault();
  });
  function menuAtCur() {
    if (sel.size === 0 && !selAll) return;
    const el = pool.find((r) => !r.hidden && r._index === cur);
    const r = (el || body).getBoundingClientRect();
    openTorrentMenu(RTL ? r.right - 40 : r.left + 40, el ? r.bottom : r.top + 20);
  }

  // ---- the phone: tap, long press, the drawer and the sheets ---------------------------
  // A tap opens the torrent's detail sheet; a long press starts the selection mode, where a
  // tap toggles a row and the toolbar acts on the selection; the row's ⋯ button opens the
  // context menu as a sheet (a visible button rather than a long-press menu: a long press is
  // the selection, as in phone apps generally). The decision docs/WEBUI.md left to slice 7.
  let lastPointer = "mouse";
  let press = null; // the row press in progress: {el, x, y, mb, long, timer}
  let drawerOpen = false;
  function phoneDown(ev) {
    cancelPress();
    if (ev.button !== 0) return;
    const el = rowOf(ev.target);
    if (!el) return;
    const p = { el, x: ev.clientX, y: ev.clientY, mb: !!ev.target.closest(".mb"), long: false, timer: 0 };
    p.timer = setTimeout(() => {
      p.long = true;
      if (p.mb || !el._id) return;
      if (!isSel(el._id)) focusRow(el._index, selecting ? "add" : "set");
      selecting = true;
      updateSelClasses();
      if (navigator.vibrate) navigator.vibrate(15);
    }, LONG_PRESS_MS);
    press = p;
  }
  function cancelPress() {
    if (press) clearTimeout(press.timer);
    press = null;
  }
  body.addEventListener("pointermove", (ev) => {
    if (press && Math.hypot(ev.clientX - press.x, ev.clientY - press.y) > 10) cancelPress();
  });
  body.addEventListener("pointercancel", cancelPress); // the browser took the touch to scroll
  body.addEventListener("pointerup", () => {
    const p = press;
    cancelPress();
    if (!p || p.long || !phone || !p.el._id || p.el.hidden) return;
    const index = p.el._index;
    if (p.mb) {
      if (!isSel(p.el._id)) focusRow(index, selecting ? "add" : "set");
      openTorrentMenu(0, 0);
    } else if (selecting) focusRow(index, "add");
    else {
      focusRow(index, "set");
      openSheet();
    }
  });

  // ---- the phone's back button: one history entry per open layer ------------------------
  // The drawer, the detail sheet, the menu sheet and the dialogs each hold a history entry
  // while they are open on the phone, and Back (a popstate) closes the topmost. A layer the
  // page closes gives its entry back with history.go, a turn later, so a close followed by an
  // open (a menu replacing a menu, the next file's Add dialog) leaves the history as it was.
  let layers = []; // the open layers' close functions, the topmost last
  let backDepth = (history.state && history.state.paLayer) || 0; // our entries in the history
  let backGoing = false; // a history.go of ours whose popstate has not come yet
  let backTimer = 0;
  function layerOpened(close) {
    if (!phone || layers.includes(close)) return;
    layers.push(close);
    syncBack();
  }
  function layerClosed(close) {
    const at = layers.indexOf(close);
    if (at < 0) return;
    layers.splice(at, 1);
    if (!backTimer) backTimer = setTimeout(syncBack);
  }
  function syncBack() {
    clearTimeout(backTimer);
    backTimer = 0;
    if (backGoing) return; // its popstate syncs again
    while (backDepth < layers.length) history.pushState({ paLayer: ++backDepth }, "");
    if (backDepth > layers.length) {
      backGoing = true;
      history.go(layers.length - backDepth);
    }
  }
  window.addEventListener("popstate", (ev) => {
    backDepth = (ev.state && ev.state.paLayer) || 0;
    if (backGoing) backGoing = false;
    else while (layers.length > backDepth) layers.pop()();
    syncBack();
  });
  syncBack(); // entries left from before a reload

  function syncScrim() {
    $("scrim").hidden = !(phone && (drawerOpen || sheetOpen || (menus.length && $("menu").classList.contains("sheet"))));
  }
  function openSheet() {
    const id = idAt(cur);
    if (!id) return;
    sheetOpen = true;
    document.body.classList.add("sheet");
    layerOpened(closeSheet);
    const c = cells.get(id);
    const nameAt = viewColumns().indexOf("name");
    $("sheet-title").textContent = c && nameAt >= 0 ? c.c[nameAt] : "";
    setDetail(id);
    syncScrim();
  }
  function closeSheet() {
    if (!sheetOpen) return;
    sheetOpen = false;
    document.body.classList.remove("sheet");
    layerClosed(closeSheet);
    if (phone) setDetail(null);
    syncScrim();
  }
  const closeDrawer = () => setDrawer(false);
  function setDrawer(open) {
    drawerOpen = open && phone;
    document.body.classList.toggle("drawer", drawerOpen);
    if (drawerOpen) layerOpened(closeDrawer);
    else layerClosed(closeDrawer);
    $("tb-menu").setAttribute("aria-expanded", String(drawerOpen));
    syncScrim();
  }
  $("tb-menu").addEventListener("click", () => setDrawer(!drawerOpen));
  $("sheet-close").addEventListener("click", closeSheet);
  $("scrim").addEventListener("click", () => {
    setDrawer(false);
    closeSheet();
  });
  $("tb-seldone").addEventListener("click", () => {
    selecting = false;
    sel = new Set();
    selAll = false;
    cur = -1;
    updateSelClasses();
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key !== "Escape" || !phone || menus.length) return;
    if (drawerOpen) setDrawer(false);
    else if (sheetOpen) closeSheet();
  });
  /// The toolbar's ⋯: what the phone toolbar has no room for, and the sort the header gives
  /// the desktop.
  $("tb-more").addEventListener("click", (ev) => {
    if (!hello) return;
    const none = selectedCount() === 0;
    const sortBy = hello.columns.map((c) => ({
      label: c.title,
      chk: sort[0] === c.id,
      run: () => setSort([c.id, sort[0] === c.id ? sort[1] : !!c.desc]),
    }));
    const r = ev.currentTarget.getBoundingClientRect();
    openMenu(
      [
        { label: T("MenuAddUrl"), run: () => $("tb-add-url").click() },
        "-",
        { label: T("MenuStop"), dis: none, run: () => command("stop") },
        { label: T("MenuQueueUp"), dis: none, run: () => command("queue_up") },
        { label: T("MenuQueueDown"), dis: none, run: () => command("queue_down") },
        "-",
        {
          label: T("MenuSelectAll"),
          dis: total === 0,
          run: () => {
            selAll = true;
            selecting = true;
            for (const id of ids) sel.add(id);
            updateSelClasses();
          },
        },
        { label: T("WebSortBy"), sub: sortBy },
        { label: T("WebSortDescending"), chk: !!sort[1], run: () => setSort([sort[0], !sort[1]]) },
        "-",
        { label: T("MenuPreferences"), run: () => $("tb-prefs").click() },
        ...(hello && hello.hosted ? [] : [{ label: T("MenuRss"), run: () => openRss(0) }]),
        { label: T("WebLogOut"), run: () => $("logout").click() },
      ],
      RTL ? r.right : r.left,
      r.bottom
    );
  });
  /// Switching between the phone and desktop layouts (a rotation, a resized window): the row
  /// height, the subscribed columns and where the detail lives all change.
  PHONE.addEventListener("change", () => {
    phone = PHONE.matches;
    if (!phone) {
      layers = []; // the desktop keeps no history entries
      syncBack();
    }
    rowH = phone ? PHONE_ROW_H : ROW_H;
    closeMenu();
    setDrawer(false);
    sheetOpen = false;
    document.body.classList.remove("sheet");
    selecting = false;
    syncScrim();
    colsGen++;
    cells = new Map(); // indexed by the old column list; the new view refills them
    if (!hello) return;
    setDetail(phone ? null : idAt(cur));
    updateSpacer();
    render();
    updateSelStatus();
    if (cur >= 0) ensureVisible(cur);
    sentWindow = null;
    sendView();
  });

  // ---- header: sort, reorder, resize, chooser -------------------------------------------
  const hdr = $("hdr");
  hdr.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    const hc = ev.target.closest(".hc");
    if (!hc) return;
    const id = hc.dataset.col;
    if (ev.target.classList.contains("rs")) {
      startResize(ev, id);
      return;
    }
    // click sorts, a drag past a threshold reorders
    const x0 = ev.clientX;
    let dragging = false;
    let ghost = null;
    let target = null;
    let after = false;
    const move = (e) => {
      if (!dragging) {
        if (Math.abs(e.clientX - x0) < 5) return;
        dragging = true;
        document.body.classList.add("drag-col");
        hc.classList.add("dragging");
        ghost = document.createElement("div");
        ghost.id = "ghost";
        ghost.textContent = colById.get(id).title;
        document.body.appendChild(ghost);
      }
      ghost.style.left = e.clientX + 8 + "px";
      ghost.style.top = e.clientY - 10 + "px";
      if (target) target.classList.remove("drop-before", "drop-after");
      target = null;
      for (const h of hdrRow.children) {
        const r = h.getBoundingClientRect();
        if (e.clientX >= r.left && e.clientX < r.right) {
          target = h;
          after = RTL ? e.clientX < (r.left + r.right) / 2 : e.clientX > (r.left + r.right) / 2;
          h.classList.add(after ? "drop-after" : "drop-before");
          break;
        }
      }
    };
    const up = () => {
      hdr.removeEventListener("pointermove", move);
      hdr.removeEventListener("pointerup", up);
      hdr.removeEventListener("pointercancel", up);
      if (!dragging) {
        clickSort(id);
        return;
      }
      document.body.classList.remove("drag-col");
      hc.classList.remove("dragging");
      if (ghost) ghost.remove();
      if (target) {
        target.classList.remove("drop-before", "drop-after");
        const to = target.dataset.col;
        if (to !== id) {
          const order = layout.order.filter((c) => c !== id);
          let at = order.indexOf(to);
          if (after) at++;
          order.splice(at, 0, id);
          layout.order = order;
          saveLayout();
          applyColumns(true);
        }
      }
    };
    hdr.setPointerCapture(ev.pointerId);
    hdr.addEventListener("pointermove", move);
    hdr.addEventListener("pointerup", up);
    hdr.addEventListener("pointercancel", up);
  });
  function clickSort(id) {
    const c = colById.get(id);
    setSort(sort && sort[0] === id ? [id, !sort[1]] : [id, !!c.desc]);
  }
  function setSort(s) {
    sort = s;
    store.set("pa.sort", sort);
    applyColumns(false);
    sentWindow = null;
    sendView();
  }
  function startResize(ev, id) {
    const w0 = widthOf(id);
    const x0 = ev.clientX;
    document.body.classList.add("drag-size");
    const move = (e) => {
      layout.widths[id] = Math.max(20, w0 + (RTL ? x0 - e.clientX : e.clientX - x0));
      applyColumns(false);
    };
    const up = () => {
      hdr.removeEventListener("pointermove", move);
      hdr.removeEventListener("pointerup", up);
      hdr.removeEventListener("pointercancel", up);
      document.body.classList.remove("drag-size");
      saveLayout();
    };
    hdr.setPointerCapture(ev.pointerId);
    hdr.addEventListener("pointermove", move);
    hdr.addEventListener("pointerup", up);
    hdr.addEventListener("pointercancel", up);
  }
  hdr.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    if (!hello) return;
    const hiddenSet = new Set(layout.hidden);
    const items = hello.columns.map((c) => ({
      label: c.title,
      chk: !hiddenSet.has(c.id),
      dis: c.id === "name",
      run: () => {
        if (hiddenSet.has(c.id)) hiddenSet.delete(c.id);
        else hiddenSet.add(c.id);
        layout.hidden = [...hiddenSet];
        saveLayout();
        applyColumns(true);
      },
    }));
    items.push("-", {
      label: T("WebResetColumns"),
      run: () => {
        layout = { order: [], widths: {}, hidden: null };
        mergeColumns(hello.columns);
        saveLayout();
        applyColumns(true);
      },
    });
    openMenu(items, ev.clientX, ev.clientY);
  });

  // ---- category tree -------------------------------------------------------------------
  function renderTree() {
    const tree = $("tree");
    tree.textContent = "";
    for (const c of hello.categories) {
      const it = document.createElement("div");
      it.className = "item";
      it.dataset.cat = c.id;
      const t = document.createElement("span");
      t.className = "t";
      t.textContent = c.title;
      const n = document.createElement("span");
      n.className = "n";
      it.append(t, n);
      tree.appendChild(it);
    }
    const g = document.createElement("div");
    g.className = "group";
    g.textContent = T("CatLabels");
    tree.appendChild(g);
    // "No Label" first, as in the 2.2.1 tree; the counts come with the ticks
    for (const l of ["", ...labels]) {
      const it = document.createElement("div");
      it.className = "item";
      it.dataset.label = l;
      const t = document.createElement("span");
      t.className = "t";
      t.textContent = l || T("CatNoLabel");
      t.title = t.textContent;
      const n = document.createElement("span");
      n.className = "n";
      n.textContent = labelCounts.has(l) ? String(labelCounts.get(l)) : "";
      it.append(t, n);
      tree.appendChild(it);
    }
    rssRenderTree();
    markTree();
  }
  function markTree() {
    for (const it of $("tree").querySelectorAll(".item")) {
      const on = filter.label != null ? it.dataset.label === filter.label : it.dataset.cat === filter.category;
      it.classList.toggle("on", on);
    }
  }
  /// The tick's Labels branch: the names when they changed (another client or the qBittorrent
  /// shim may add or remove one), then every count.
  function updateLabels(list, noLabel) {
    labelCounts = new Map([["", noLabel], ...list]);
    const names = list.map((x) => x[0]);
    if (names.length !== labels.length || names.some((l, i) => l !== labels[i])) {
      labels = names;
      renderTree();
      if (filter.label && !labels.includes(filter.label)) {
        // the selected label went away: show everything
        filter = { category: "all", label: null };
        store.set("pa.filter", filter);
        markTree();
        body.scrollTop = 0;
        sentWindow = null;
        sendView();
      }
      return;
    }
    for (const it of $("tree").querySelectorAll(".item[data-label]")) {
      const n = labelCounts.get(it.dataset.label);
      const text = n === undefined ? "" : String(n);
      if (it.lastChild.textContent !== text) it.lastChild.textContent = text;
    }
  }
  function updateCounts(counts) {
    for (const it of $("tree").querySelectorAll(".item[data-cat]")) {
      const n = counts[it.dataset.cat];
      const el = it.lastChild;
      const text = n === undefined ? "" : String(n);
      if (el.textContent !== text) el.textContent = text;
    }
  }
  $("tree").addEventListener("click", (ev) => {
    const it = ev.target.closest(".item");
    if (!it) return;
    filter =
      it.dataset.label !== undefined
        ? { category: "all", label: it.dataset.label }
        : { category: it.dataset.cat, label: null };
    store.set("pa.filter", filter);
    markTree();
    setDrawer(false);
    body.scrollTop = 0;
    sentWindow = null;
    sendView();
  });

  // ---- status bar ----------------------------------------------------------------------
  function updateStatus(s) {
    setText("st-dht", T("BarDht", { count: s.dht }));
    // the global limit in brackets while one is set
    const limit = (rate, l) => rate + (l ? " [" + l + "]" : "");
    setText("st-down", T("BarDown", { rate: limit(s.down, s.down_limit), total: s.down_total }));
    setText("st-up", T("BarUp", { rate: limit(s.up, s.up_limit), total: s.up_total }));
  }
  function setText(id, text) {
    const el = $(id);
    if (el.textContent !== text) el.textContent = text;
  }

  // ---- search --------------------------------------------------------------------------
  let searchTimer = 0;
  $("search").addEventListener("input", (ev) => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      search = ev.target.value.trim();
      body.scrollTop = 0;
      sentWindow = null;
      sendView();
    }, 150);
  });
  $("search").addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.target.value = "";
      ev.target.dispatchEvent(new Event("input"));
      body.focus();
    } else if (ev.key === "Enter") body.focus();
  });

  // ---- commands -------------------------------------------------------------------------
  /// The view's filter as the server sees it (the `view` subscription's).
  function viewFilter() {
    return { category: filter.label != null ? "all" : filter.category || "all", label: filter.label ?? null, search };
  }
  /// The selection for a command body: with Ctrl+A, "all" with the view's filter, so the
  /// server acts on every matching torrent, not just the loaded window; else the ids.
  function selection() {
    if (selAll) return { all: true, filter: viewFilter() };
    return sel.size ? { ids: [...sel] } : null;
  }
  function selectedCount() {
    return selAll ? total : sel.size;
  }
  /// The Remove question for the selection: by name for one torrent, else by count.
  function removeQuestion(data) {
    let name = null;
    if (selectedCount() === 1 && !selAll) {
      const id = [...sel][0];
      const c = cells.get(id);
      const nameAt = viewColumns().indexOf("name");
      if (c && nameAt >= 0) name = c.c[nameAt];
    }
    const q =
      name !== null
        ? T(data ? "AskRemoveData" : "AskRemove", { name })
        : T(data ? "AskRemoveDataMany" : "AskRemoveMany", { count: selectedCount() });
    return q + "\n" + T(data ? "RemoveDeletesDataTrash" : "RemoveKeepsData");
  }
  async function command(name, arg) {
    const body = selection();
    if (!body) return;
    let path;
    switch (name) {
      case "start":
      case "force_start":
      case "pause":
      case "stop":
      case "recheck":
      case "reannounce":
        path = "torrents/" + name.replace("_", "-");
        break;
      case "remove":
      case "remove_data": {
        const data = name === "remove_data";
        const msg = removeQuestion(data);
        if (confirmDelete && !confirm(msg)) return;
        path = "torrents/remove";
        body.data = data;
        sel = new Set();
        selAll = false;
        cur = -1;
        closeSheet();
        setDetail(null);
        updateSelClasses();
        break;
      }
      case "queue_up":
      case "queue_down":
      case "queue_top":
      case "queue_bottom":
        path = "torrents/queue";
        body.move = name.slice(6);
        break;
      case "superseed":
        path = "torrents/super-seed";
        body.on = !!arg;
        break;
      case "dl_limit":
        path = "torrents/limits";
        body.down = arg * 1024;
        break;
      case "ul_limit":
        path = "torrents/limits";
        body.up = arg * 1024;
        break;
      case "label":
        path = "torrents/label";
        body.label = arg || "";
        break;
      case "label_new": {
        const l = prompt(T("NewLabelPrompt"));
        if (!l || !l.trim()) return;
        const label = l.trim();
        path = "torrents/label";
        body.label = label;
        if (!labels.includes(label)) {
          labels.push(label);
          labels.sort();
          renderTree();
        }
        break;
      }
      case "location": {
        const dir = prompt(T("WebSetLocation"));
        if (!dir || !dir.trim()) return;
        path = "torrents/location";
        body.path = dir.trim();
        break;
      }
      default:
        return;
    }
    try {
      await pa(path, body);
    } catch (e) {
      /* shown by pa() */
    }
  }
  document.querySelectorAll("#toolbar [data-cmd]").forEach((b) => {
    b.addEventListener("click", () => command(b.dataset.cmd));
  });

  // ---- the torrent context menu, in the Win32 window's order ----------------------------
  function openTorrentMenu(x, y) {
    const n = selectedCount();
    if (n === 0) return;
    const dis = (hint) => ({ dis: true, hint });
    const items = [
      { label: T("MenuOpenFolder"), ...dis(T("WebNotFromBrowser")) },
      "-",
      { label: T("MenuForceStart"), run: () => command("force_start") },
      { label: T("MenuStart"), run: () => command("start") },
      { label: T("MenuPause"), run: () => command("pause") },
      { label: T("MenuStop"), run: () => command("stop") },
      "-",
      { label: T("MenuQueueUp"), run: () => command("queue_up") },
      { label: T("MenuQueueDown"), run: () => command("queue_down") },
      { label: T("MenuQueueTop"), run: () => command("queue_top") },
      { label: T("MenuQueueBottom"), run: () => command("queue_bottom") },
      "-",
      { label: T("MenuRemove"), run: () => command("remove") },
      { label: T("MenuRemoveAnd"), sub: [{ label: T("MenuDeleteData"), run: () => command("remove_data") }] },
      "-",
      { label: T("MenuRecheck"), run: () => command("recheck") },
      { label: T("MenuUpdateTracker"), run: () => command("reannounce") },
      "-",
      {
        label: T("MenuAdvanced"),
        sub: [
          { label: T("MenuSetLocation"), run: () => command("location") },
          { label: T("MenuInitialSeeding"), key: "superseed", run: (it) => command("superseed", !it.chk) },
        ],
      },
      {
        label: T("MenuBandwidth"),
        sub: [
          { label: T("MenuSetDownLimit"), sub: limitItems("dl") },
          { label: T("MenuSetUpLimit"), sub: limitItems("ul") },
        ],
      },
      {
        label: T("MenuLabel"),
        sub: [
          { label: T("MenuNewLabel"), run: () => command("label_new") },
          { label: T("MenuRemoveLabel"), key: "label:", run: () => command("label", "") },
          ...(labels.length ? ["-"] : []),
          ...labels.map((l) => ({ label: l, key: "label:" + l, run: () => command("label", l) })),
        ],
      },
      "-",
      { label: T("MenuProperties"), run: () => openProps(false) },
    ];
    // the phone's sheet: no folder to open on a phone; the detail sheet first (a tap opens it
    // too, but not in selection mode)
    if (phone) items.splice(0, 2);
    if (phone && n === 1 && !selAll) items.unshift({ label: T("WebDetails"), run: openSheet }, "-");
    openMenu(items, x, y);
    fillChecks(items);
  }
  function limitItems(kind) {
    return LIMITS.map((kb) => ({
      label: kb === 0 ? T("MenuUnlimited") : kb + " " + T("UnitKbPerSec"),
      key: kind + ":" + kb,
      run: () => command(kind + "_limit", kb),
    }));
  }
  /// Check marks need the torrents' current limits, label and super-seeding flag, which the
  /// cells do not carry: one shim read (`torrents/info`), applied while the menu is still open.
  async function fillChecks(items) {
    if (selAll || sel.size === 0 || sel.size > 100) return;
    const hashes = [...sel].join("|");
    let info;
    try {
      info = JSON.parse(await api("torrents/info?hashes=" + encodeURIComponent(hashes)));
    } catch (e) {
      return;
    }
    if (!Array.isArray(info) || !info.length) return;
    const same = (f) => {
      const v = f(info[0]);
      return info.every((t) => f(t) === v) ? v : undefined;
    };
    const checks = new Map();
    checks.set("superseed", same((t) => !!t.super_seeding) === true);
    const dl = same((t) => t.dl_limit);
    const ul = same((t) => t.up_limit);
    for (const kb of LIMITS) {
      checks.set("dl:" + kb, dl === kb * 1024 || (kb === 0 && dl !== undefined && dl <= 0));
      checks.set("ul:" + kb, ul === kb * 1024 || (kb === 0 && ul !== undefined && ul <= 0));
    }
    const label = same((t) => t.category || "");
    checks.set("label:", false);
    for (const l of labels) checks.set("label:" + l, label === l);
    const grey = new Map([["label:", label === ""]]);
    const walk = (list) => {
      for (const it of list) {
        if (typeof it !== "object") continue;
        if (it.key && checks.has(it.key)) {
          it.chk = checks.get(it.key);
          if (it.el) it.el.classList.toggle("chk", it.chk);
        }
        if (it.key && grey.has(it.key)) {
          it.dis = grey.get(it.key);
          if (it.el) it.el.classList.toggle("dis", it.dis);
        }
        if (it.sub) walk(it.sub);
      }
    };
    walk(items);
  }

  // ---- menus ---------------------------------------------------------------------------
  const menus = []; // open menu elements, root first
  /// Where menus live: inside the topmost open dialog (a modal dialog's top layer covers the
  /// page and makes the rest of it inert), else the body.
  function menuHost() {
    const open = document.querySelectorAll("dialog[open]");
    return open.length ? open[open.length - 1] : document.body;
  }
  function openMenu(items, x, y) {
    closeMenu();
    const root = $("menu");
    const host = menuHost();
    if (root.parentNode !== host) host.appendChild(root);
    buildMenu(root, items);
    showMenuAt(root, x, y);
    menus.push(root);
    layerOpened(closeMenu);
    syncScrim();
    setTimeout(() => {
      document.addEventListener("pointerdown", onDocPointer, true);
      document.addEventListener("keydown", onMenuKey, true);
    });
    window.addEventListener("blur", closeMenu, { once: true });
  }
  function buildMenu(el, items) {
    el.textContent = "";
    el.hidden = false;
    // a phone's menu is a sheet from the bottom, and a submenu replaces it (with a way back)
    el.className = "menu" + (phone ? " sheet" : "");
    el.setAttribute("role", "menu");
    el._items = items;
    for (const it of items) {
      if (it === "-") {
        const s = document.createElement("div");
        s.className = "ms";
        el.appendChild(s);
        continue;
      }
      const mi = document.createElement("div");
      mi.className = "mi" + (it.dis ? " dis" : "") + (it.chk ? " chk" : "") + (it.sub ? " sub" : "") + (it.back ? " back" : "");
      mi.setAttribute("role", "menuitem");
      mi.textContent = it.label;
      if (it.hint) mi.title = it.hint;
      it.el = mi;
      mi._item = it;
      mi.addEventListener("pointerenter", () => {
        if (phone) return; // a touch has no hover: a tap opens the submenu
        highlight(el, mi);
        if (it.sub && !it.dis) openSub(el, mi, it.sub);
        else closeSubsOf(el);
      });
      mi.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (it.dis) return;
        if (it.back) {
          buildMenu(el, it.back);
          return;
        }
        if (it.sub && phone) {
          buildMenu(el, [{ label: "‹ " + it.label, back: el._items }, "-", ...it.sub]);
          return;
        }
        if (it.sub) {
          openSub(el, mi, it.sub);
          return;
        }
        closeMenu();
        it.run(it);
      });
      el.appendChild(mi);
    }
  }
  function showMenuAt(el, x, y) {
    el.hidden = false;
    el.style.left = "0px";
    el.style.top = "0px";
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    if (RTL) x = Math.max(0, x - w); // right to left, a menu opens leftwards from x
    if (x + w > innerWidth) x = Math.max(0, innerWidth - w - 2);
    if (y + h > innerHeight) y = Math.max(0, innerHeight - h - 2);
    el.style.left = x + "px";
    el.style.top = y + "px";
  }
  function highlight(menu, mi) {
    for (const c of menu.children) c.classList.toggle("open", c === mi);
  }
  function openSub(parent, mi, items) {
    closeSubsOf(parent);
    const sub = document.createElement("div");
    menuHost().appendChild(sub);
    buildMenu(sub, items);
    const r = mi.getBoundingClientRect();
    let x = r.right - 2;
    if (RTL) {
      x = r.left + 2;
      if (x - sub.offsetWidth < 0) x = r.right - 2 + sub.offsetWidth;
    } else if (x + sub.offsetWidth > innerWidth) x = Math.max(0, r.left - sub.offsetWidth + 2);
    showMenuAt(sub, x, r.top - 3);
    sub._parent = parent;
    menus.push(sub);
  }
  function closeSubsOf(menu) {
    const at = menus.indexOf(menu);
    if (at < 0) return;
    for (const m of menus.splice(at + 1)) m.remove();
  }
  function closeMenu() {
    for (const m of menus.splice(1)) m.remove();
    const root = menus.pop();
    if (root) {
      root.hidden = true;
      root.textContent = "";
    }
    document.removeEventListener("pointerdown", onDocPointer, true);
    document.removeEventListener("keydown", onMenuKey, true);
    layerClosed(closeMenu);
    syncScrim();
  }
  function onDocPointer(ev) {
    if (!ev.target.closest(".menu")) closeMenu();
  }
  function onMenuKey(ev) {
    const menu = menus[menus.length - 1];
    if (!menu) return;
    const items = [...menu.children].filter((c) => c.classList.contains("mi"));
    const at = items.findIndex((c) => c.classList.contains("open"));
    const step = (d) => {
      let i = at;
      for (let n = 0; n < items.length; n++) {
        i = (i + d + items.length) % items.length;
        if (!items[i].classList.contains("dis")) break;
      }
      highlight(menu, items[i]);
    };
    // right to left, a submenu opens towards the left
    const flip = { ArrowLeft: "ArrowRight", ArrowRight: "ArrowLeft" };
    switch (RTL ? flip[ev.key] || ev.key : ev.key) {
      case "Escape":
        if (menus.length > 1) {
          menus.pop().remove();
        } else closeMenu();
        break;
      case "ArrowDown":
        step(1);
        break;
      case "ArrowUp":
        step(-1);
        break;
      case "ArrowRight":
        if (at >= 0 && items[at]._item.sub) {
          openSub(menu, items[at], items[at]._item.sub);
          const sub = menus[menus.length - 1];
          const first = sub.querySelector(".mi:not(.dis)");
          if (first) highlight(sub, first);
        }
        break;
      case "ArrowLeft":
        if (menus.length > 1) menus.pop().remove();
        break;
      case "Enter":
      case " ":
        if (at >= 0) items[at].click();
        break;
      default:
        return;
    }
    ev.preventDefault();
    ev.stopPropagation();
  }

  // ---- detail tabs ---------------------------------------------------------------------
  // One subscription at a time (the focused row and the active tab). The server sends the
  // tab's whole content once, then only when it changed (Speed and Logger append). Each tab
  // keeps its DOM between updates, so the scroll position, the sort and the selection stay.
  const tabBody = $("tab-body");
  let detailView = null; // {tab, id, ...} the DOM built for the current subscription
  const tabSort = { trackers: [0, false], peers: [0, false], pieces: [0, false], files: [0, false] };
  const tabSel = { trackers: new Set(), peers: new Set(), pieces: new Set(), files: new Set() };
  const DOWN_COLOR = "#2aa52a";
  const UP_COLOR = "#d23c3c";

  function clearDetail() {
    detailView = null;
    tabBody.textContent = "";
    tabBody.scrollTop = 0;
  }
  function onDetail(m) {
    if (m.id !== detailId || m.tab !== detailTab) return; // a message for an old subscription
    if (m.gone) {
      clearDetail();
      return;
    }
    if (!detailView || detailView.tab !== m.tab || detailView.id !== m.id) {
      clearDetail();
      detailView = buildTab(m.tab, m.id);
      tabBody.appendChild(detailView.el);
    }
    detailView.update(m);
  }
  function buildTab(tab, id) {
    switch (tab) {
      case "general":
        return generalView(tab, id);
      case "trackers":
      case "peers":
      case "files":
        return tableView(tab, id);
      case "pieces":
        return piecesView(tab, id);
      case "speed":
        return speedView(tab, id);
      case "logger":
        return loggerView(tab, id);
      default:
        return { tab, id, el: document.createElement("div"), update() {} };
    }
  }
  function setTextIf(el, text) {
    if (el.textContent !== text) el.textContent = text;
  }

  // General: the Downloaded and Availability bars, then the Transfer and General groups as
  // two columns of label-value pairs.
  function generalView(tab, id) {
    const el = document.createElement("div");
    el.className = "gen";
    const bars = [];
    const groups = [];
    return {
      tab,
      id,
      el,
      update(m) {
        (m.bars || []).forEach(([label, value, fill], i) => {
          let b = bars[i];
          if (!b) {
            const row = document.createElement("div");
            row.className = "bar-row";
            const l = document.createElement("b");
            const bar = document.createElement("div");
            bar.className = "pbar" + (i === 1 ? " avail" : "");
            bar.appendChild(document.createElement("i"));
            const v = document.createElement("span");
            row.append(l, bar, v);
            el.appendChild(row);
            b = bars[i] = { l, bar, v };
          }
          setTextIf(b.l, label);
          setTextIf(b.v, value);
          b.bar.firstChild.style.width = fill / 10 + "%";
        });
        (m.groups || []).forEach((g, gi) => {
          let grp = groups[gi];
          if (!grp) {
            const h = document.createElement("h4");
            const pairs = document.createElement("div");
            pairs.className = "pairs";
            el.append(h, pairs);
            grp = groups[gi] = { h, pairs, rows: [] };
          }
          setTextIf(grp.h, g.title);
          g.rows.forEach(([label, value], ri) => {
            let r = grp.rows[ri];
            if (!r) {
              const p = document.createElement("div");
              p.className = "pair";
              const l = document.createElement("b");
              const v = document.createElement("span");
              p.append(l, v);
              grp.pairs.appendChild(p);
              r = grp.rows[ri] = { l, v };
            }
            setTextIf(r.l, label);
            setTextIf(r.v, value);
            if (r.v.title !== value) r.v.title = value;
          });
        });
      },
    };
  }

  // Trackers, Peers, Pieces, Files: a table with the frontend's columns, sortable by header
  // click (client-side; these lists are small), rows reconciled by key so nothing flickers.
  // `blocksCol` is a column drawn from the row's block letters (the Pieces list) instead of text.
  function tableView(tab, id, blocksCol = -1) {
    const cols = (hello && hello.detail && hello.detail[tab]) || [];
    const keyOf = tab === "files" ? (r) => String(r.i) : (r) => r.c[0];
    const el = document.createElement("table");
    el.className = "dt";
    const colgroup = document.createElement("colgroup");
    const thead = document.createElement("thead");
    const hr = document.createElement("tr");
    cols.forEach((c, i) => {
      const col = document.createElement("col");
      col.style.width = c.width + "px";
      colgroup.appendChild(col);
      const th = document.createElement("th");
      th.textContent = c.title;
      th.title = c.title;
      if (c.right) th.className = "r";
      th.dataset.i = i;
      hr.appendChild(th);
    });
    thead.appendChild(hr);
    const tbody = document.createElement("tbody");
    el.append(colgroup, thead, tbody);
    const byKey = new Map();
    let rows = [];
    const markSort = () => {
      const [si, desc] = tabSort[tab];
      for (const th of hr.children) {
        const on = Number(th.dataset.i) === si;
        th.classList.toggle("sort", on);
        th.classList.toggle("desc", on && desc);
        th.classList.toggle("asc", on && !desc);
      }
    };
    const sorted = () => {
      const [si, desc] = tabSort[tab];
      const out = rows.slice();
      out.sort((a, b) => {
        const ka = a.k && a.k[si];
        const kb = b.k && b.k[si];
        let r;
        if (typeof ka === "number" && typeof kb === "number") r = ka - kb;
        else if (typeof ka === "number") r = 1; // numbers after blanks? no: blanks last
        else if (typeof kb === "number") r = -1;
        else r = (a.c[si] || "").localeCompare(b.c[si] || "", undefined, { numeric: true, sensitivity: "base" });
        if (r === 0 && si !== 0) r = (a.c[0] || "").localeCompare(b.c[0] || "", undefined, { numeric: true });
        return desc ? -r : r;
      });
      return out;
    };
    const render = () => {
      const list = sorted();
      const keep = new Set();
      list.forEach((r, i) => {
        const key = keyOf(r);
        keep.add(key);
        let tr = byKey.get(key);
        if (!tr) {
          tr = document.createElement("tr");
          tr.dataset.key = key;
          cols.forEach((c) => {
            const td = document.createElement("td");
            if (c.right) td.className = "r";
            tr.appendChild(td);
          });
          byKey.set(key, tr);
        }
        r.c.forEach((text, j) => {
          const td = tr.children[j];
          if (j === blocksCol) {
            if (td && td.dataset.b !== r.b) {
              td.dataset.b = r.b || "";
              drawBlocks(td, r.b || "");
            }
            return;
          }
          if (td && td.textContent !== text) {
            td.textContent = text;
            if (j === 0) td.title = text;
          }
        });
        tr.classList.toggle("sel", tabSel[tab].has(key));
        if (tbody.children[i] !== tr) tbody.insertBefore(tr, tbody.children[i] || null);
      });
      for (const [key, tr] of byKey) {
        if (!keep.has(key)) {
          tr.remove();
          byKey.delete(key);
          tabSel[tab].delete(key);
        }
      }
    };
    thead.addEventListener("click", (ev) => {
      const th = ev.target.closest("th");
      if (!th) return;
      const i = Number(th.dataset.i);
      const [si, desc] = tabSort[tab];
      tabSort[tab] = si === i ? [i, !desc] : [i, false];
      markSort();
      render();
    });
    let anchorKey = null;
    tbody.addEventListener("pointerdown", (ev) => {
      if (ev.button !== 0 && ev.button !== 2) return;
      const tr = ev.target.closest("tr");
      if (!tr) return;
      const key = tr.dataset.key;
      const sel = tabSel[tab];
      if (ev.button === 2) {
        if (!sel.has(key)) {
          sel.clear();
          sel.add(key);
          anchorKey = key;
        }
      } else if (ev.ctrlKey || ev.metaKey) {
        if (sel.has(key)) sel.delete(key);
        else sel.add(key);
        anchorKey = key;
      } else if (ev.shiftKey && anchorKey !== null) {
        const keys = [...tbody.children].map((r) => r.dataset.key);
        const a = keys.indexOf(anchorKey);
        const b = keys.indexOf(key);
        sel.clear();
        for (let i = Math.min(a, b); i <= Math.max(a, b); i++) sel.add(keys[i]);
      } else {
        sel.clear();
        sel.add(key);
        anchorKey = key;
      }
      for (const r of tbody.children) r.classList.toggle("sel", sel.has(r.dataset.key));
    });
    if (tab === "files") {
      tbody.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        const sel = tabSel[tab];
        if (!sel.size) return;
        const prio = (p) => () => {
          const priorities = {};
          for (const i of sel) priorities[i] = p;
          pa("torrents/files", { id, priorities }).catch(() => {});
        };
        openMenu(
          PRIORITIES.map(([key, p]) => ({ label: T(key), run: prio(p) })),
          ev.clientX,
          ev.clientY
        );
      });
    }
    markSort();
    return {
      tab,
      id,
      el,
      update(m) {
        rows = m.rows || [];
        render();
      },
    };
  }

  // A piece's blocks as one bar: blue on disk, green being written, orange requested, grey
  // free; a CSS gradient with a hard stop per run of one kind.
  const BLOCK_COLORS = { f: "var(--bar-dl)", w: "var(--bar-seed)", r: "var(--blk-req)", "-": "var(--bar-bg)" };
  function drawBlocks(td, letters) {
    let bar = td.firstChild;
    if (!bar) {
      bar = document.createElement("i");
      bar.className = "blk";
      td.appendChild(bar);
    }
    const n = letters.length;
    const stops = [];
    for (let i = 0; i < n; ) {
      let j = i;
      while (j < n && letters[j] === letters[i]) j++;
      const color = BLOCK_COLORS[letters[i]] || BLOCK_COLORS["-"];
      stops.push(`${color} ${((i * 100) / n).toFixed(2)}% ${((j * 100) / n).toFixed(2)}%`);
      i = j;
    }
    bar.style.backgroundImage = stops.length ? `linear-gradient(to right, ${stops.join(", ")})` : "";
    td.title = T("WebBlocksDone", { done: letters.split("f").length - 1, count: n });
  }

  // Pieces: one bar for the bitfield (blue = have, grey = missing, orange = being
  // downloaded), each pixel column shaded by the share of its pieces we have; under it the
  // pieces being downloaded with their blocks (µTorrent 2.2.1's list).
  function piecesView(tab, id) {
    const el = document.createElement("div");
    el.className = "pieces-tab";
    const head = document.createElement("div");
    head.className = "pieces";
    const text = document.createElement("div");
    text.className = "ptext";
    const canvas = document.createElement("canvas");
    head.append(text, canvas);
    const list = tableView(tab, id, 3);
    el.append(head, list.el);
    let data = null;
    const draw = () => {
      if (!data) return;
      const w = Math.max(10, head.clientWidth - 16);
      const h = 24;
      const dpr = window.devicePixelRatio || 1;
      if (canvas.width !== Math.round(w * dpr)) canvas.width = Math.round(w * dpr);
      if (canvas.height !== Math.round(h * dpr)) canvas.height = Math.round(h * dpr);
      canvas.style.width = w + "px";
      canvas.style.height = h + "px";
      const ctx = canvas.getContext("2d");
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const css = getComputedStyle(document.documentElement);
      const bg = css.getPropertyValue("--bar-bg").trim() || "#e6e6e6";
      const have = css.getPropertyValue("--bar-dl").trim() || "#3c86e0";
      ctx.fillStyle = bg;
      ctx.fillRect(0, 0, w, h);
      const n = data.n;
      if (n > 0) {
        // prefix sums of the bitfield from the run lengths, so a column's share is O(1)
        const runs = data.rle;
        const bounds = []; // [start, end) of have-runs
        let at = 0;
        let v = false;
        for (const len of runs) {
          if (v) bounds.push([at, at + len]);
          at += len;
          v = !v;
        }
        const cols = Math.min(w, n);
        const pw = w / cols;
        let bi = 0;
        for (let x = 0; x < cols; x++) {
          const p0 = Math.floor((x * n) / cols);
          const p1 = Math.max(p0 + 1, Math.floor(((x + 1) * n) / cols));
          let had = 0;
          while (bi > 0 && bounds[bi - 1][1] > p0) bi--;
          for (let j = bi; j < bounds.length && bounds[j][0] < p1; j++) {
            had += Math.max(0, Math.min(bounds[j][1], p1) - Math.max(bounds[j][0], p0));
            if (bounds[j][1] <= p0) bi = j + 1;
          }
          const share = had / (p1 - p0);
          if (share > 0) {
            ctx.globalAlpha = 0.25 + 0.75 * share;
            ctx.fillStyle = have;
            ctx.fillRect(x * pw, 0, Math.ceil(pw), h);
            ctx.globalAlpha = 1;
          }
        }
        ctx.fillStyle = "#e8a000";
        for (const p of data.downloading || []) {
          const x = Math.floor((p * cols) / n);
          ctx.fillRect(x * pw, h - 6, Math.max(2, Math.ceil(pw)), 6);
        }
      }
      ctx.strokeStyle = css.getPropertyValue("--line").trim() || "#d9d9d9";
      ctx.strokeRect(0.5, 0.5, w - 1, h - 1);
    };
    new ResizeObserver(draw).observe(head);
    return {
      tab,
      id,
      el,
      update(m) {
        data = m;
        const dl = (m.downloading || []).length;
        setTextIf(
          text,
          dl
            ? T("WebPiecesSummaryDownloading", { count: m.n, done: m.have, downloading: dl })
            : T("WebPiecesSummary", { count: m.n, done: m.have })
        );
        draw();
        list.update(m);
      },
    };
  }

  // Speed: the last 10 minutes of download and upload rates, newest at the right, on the
  // axis the server scaled (1, 2 or 5 x 10^n kB/s) with its labels.
  function speedView(tab, id) {
    const el = document.createElement("div");
    el.className = "speed";
    const canvas = document.createElement("canvas");
    el.appendChild(canvas);
    let samples = [];
    let keep = 600;
    let scale = { top: 10240, labels: [] };
    let legend = ["", ""];
    const draw = () => {
      const w = Math.max(40, el.clientWidth - 16);
      const h = Math.max(60, el.clientHeight - 12);
      const dpr = window.devicePixelRatio || 1;
      if (canvas.width !== Math.round(w * dpr)) canvas.width = Math.round(w * dpr);
      if (canvas.height !== Math.round(h * dpr)) canvas.height = Math.round(h * dpr);
      canvas.style.width = w + "px";
      canvas.style.height = h + "px";
      const ctx = canvas.getContext("2d");
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const css = getComputedStyle(document.documentElement);
      const fg = css.getPropertyValue("--fg").trim() || "#000";
      const muted = css.getPropertyValue("--muted").trim() || "#6d6d6d";
      const line = css.getPropertyValue("--line").trim() || "#d9d9d9";
      const bg = css.getPropertyValue("--bg").trim() || "#fff";
      ctx.clearRect(0, 0, w, h);
      ctx.font = "11px Segoe UI, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const left = 78;
      const top = 22;
      const bottom = h - 16;
      const right = w - 4;
      ctx.fillStyle = bg;
      ctx.fillRect(left, top, right - left, bottom - top);
      // horizontal grid at the quarters, labelled from the server's axis
      ctx.strokeStyle = line;
      ctx.lineWidth = 1;
      for (let i = 0; i <= 4; i++) {
        const y = Math.round(top + ((bottom - top) * i) / 4) + 0.5;
        if (i > 0 && i < 4) {
          ctx.beginPath();
          ctx.moveTo(left, y);
          ctx.lineTo(right, y);
          ctx.stroke();
        }
        const label = i < 4 ? scale.labels[i] : "";
        if (label) {
          ctx.fillStyle = muted;
          ctx.textAlign = "right";
          ctx.fillText(label, left - 4, y);
        }
      }
      // the time axis: one minute per division, 10 minutes across
      const minutes = keep / 60;
      ctx.textAlign = "center";
      for (let m = 0; m <= minutes; m++) {
        const x = Math.round(right - ((right - left) * m) / minutes) + 0.5;
        if (m > 0 && m < minutes) {
          ctx.strokeStyle = line;
          ctx.beginPath();
          ctx.moveTo(x, top);
          ctx.lineTo(x, bottom);
          ctx.stroke();
        }
        if (m % 5 === 0 || m === minutes) {
          ctx.fillStyle = muted;
          ctx.fillText(m === 0 ? "now" : m + " min", x, bottom + 8);
        }
      }
      ctx.strokeStyle = muted;
      ctx.strokeRect(left + 0.5, top + 0.5, right - left - 1, bottom - top - 1);
      // the two lines, one sample per 1/keep of the width, newest at the right edge
      const n = samples.length;
      if (n >= 2) {
        const step = (right - left - 2) / (keep - 1);
        [0, 1].forEach((series) => {
          ctx.strokeStyle = series === 0 ? DOWN_COLOR : UP_COLOR;
          ctx.lineWidth = 2;
          ctx.beginPath();
          for (let i = 0; i < n; i++) {
            const x = right - 1 - (n - 1 - i) * step;
            const y = bottom - 1 - (Math.min(samples[i][series], scale.top) / scale.top) * (bottom - top - 2);
            if (x < left + 1) continue;
            if (i === 0 || x - step < left + 1) ctx.moveTo(x, y);
            else ctx.lineTo(x, y);
          }
          ctx.stroke();
        });
      }
      // legend
      let x = left;
      ctx.textAlign = "left";
      [DOWN_COLOR, UP_COLOR].forEach((color, i) => {
        ctx.fillStyle = color;
        ctx.fillRect(x, 6, 10, 10);
        ctx.fillStyle = fg;
        ctx.fillText(legend[i] || "", x + 14, 11);
        x += 200;
      });
    };
    new ResizeObserver(draw).observe(el);
    return {
      tab,
      id,
      el,
      update(m) {
        if (m.all) {
          samples = m.samples || [];
          if (m.keep) keep = m.keep;
        } else if (m.samples) {
          samples.push(...m.samples);
          if (samples.length > keep) samples.splice(0, samples.length - keep);
        }
        if (m.scale) scale = m.scale;
        if (m.legend) legend = m.legend;
        draw();
      },
    };
  }

  // Logger: the alert lines for the torrent and the session, newest at the bottom; the view
  // follows the tail unless the user scrolled up.
  function loggerView(tab, id) {
    const el = document.createElement("div");
    el.className = "log";
    const MAX = 500;
    const hms = (t) => {
      const d = new Date(t * 1000);
      const p = (n) => String(n).padStart(2, "0");
      return `[${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}]`;
    };
    return {
      tab,
      id,
      el,
      update(m) {
        const atBottom = tabBody.scrollTop + tabBody.clientHeight >= tabBody.scrollHeight - 4;
        if (m.all) el.textContent = "";
        for (const [t, text] of m.lines || []) {
          const d = document.createElement("div");
          d.textContent = hms(t) + " " + text;
          el.appendChild(d);
        }
        while (el.children.length > MAX) el.firstChild.remove();
        if (m.all || atBottom) tabBody.scrollTop = tabBody.scrollHeight;
      },
    };
  }

  $("tabs").addEventListener("click", (ev) => {
    const b = ev.target.closest("[data-tab]");
    if (!b || b.dataset.tab === detailTab) return;
    detailTab = b.dataset.tab;
    store.set("pa.tab", detailTab);
    for (const t of $("tabs").children) t.classList.toggle("on", t === b);
    clearDetail();
    if (detailId) send({ t: "detail", id: detailId, tab: detailTab });
  });
  for (const t of $("tabs").children) t.classList.toggle("on", t.dataset.tab === detailTab);

  // ---- adding: the Add New Torrent dialog --------------------------------------------------
  // A .torrent (the Add Torrent button, a drop) is parsed by the server first (pa/torrent-info)
  // and shown in the 2.2.1 dialog: Save In, Name, the file tree with check boxes and
  // priorities, Skip hash check, Start torrent, Add to top of queue, Label; OK uploads it with
  // the choices (pa/add). A magnet (Add URL, a drop, a paste) joins the session in upload mode
  // to fetch its metadata (pa/add with dialog); once its file list is known the tick's `adds`
  // changes, GET pa/add lists it as ready and the same dialog opens for it (pa/torrent-info?
  // hash=); OK sends the choices with its hash, Cancel removes it. Other URLs add directly.
  const addDlg = $("add");
  const addQueue = []; // files, and {hash} of waiting magnets, waiting for their turn
  let addState = null; // {file, info, tree, sel, anchor, done}; file is {hash} for a magnet
  const shownAdds = new Set(); // waiting magnets queued or shown here
  let addsVersion = 0;

  /// frontend::fmt::size: binary units, three significant digits, "kB".
  function fmtSize(bytes) {
    const units = ["UnitB", "UnitKb", "UnitMb", "UnitGb", "UnitTb", "UnitPb"].map((k) => T(k));
    if (bytes < 1024) return bytes + " " + units[0];
    let v = bytes;
    let u = 0;
    while (v >= 999.5 && u + 1 < units.length) {
      v /= 1024;
      u++;
    }
    const d = v >= 99.95 ? 0 : v >= 9.995 ? 1 : 2;
    return v.toFixed(d) + " " + units[u];
  }
  function priorityText(p) {
    return T(p === 0 ? "PrioDontDownload" : p <= 3 ? "PrioLow" : p === 4 ? "PrioNormal" : "PrioHigh");
  }

  $("tb-add").addEventListener("click", () => $("file").click());
  $("file").addEventListener("change", (ev) => {
    const files = [...ev.target.files];
    ev.target.value = "";
    queueTorrents(files);
  });
  $("tb-add-url").addEventListener("click", () => {
    const url = prompt(T("AddUrlTitle") + "\n\n" + T("AddUrlPrompt"));
    if (!url || !url.trim()) return;
    addUrl(url.trim());
  });
  async function addUrl(url) {
    const magnet = /^magnet:/i.test(url) || /^[0-9a-f]{40}$/i.test(url) || /^[a-z2-7]{32}$/i.test(url);
    if (magnet) {
      let setup = null;
      try {
        setup = await pa("add");
      } catch (e) {
        /* the dialog then */
      }
      if (!setup || setup.show_dialog !== false) {
        pa("add", { url, dialog: true }).catch(() => {});
        return;
      }
    }
    pa("add", { url }).catch(() => {});
  }
  /// The tick's `adds`: the waiting magnets changed; queue the ready ones not seen here yet,
  /// and drop (or close) the ones finished elsewhere.
  function addsChanged(v) {
    if (v === addsVersion) return;
    addsVersion = v;
    loadReadyAdds();
  }
  async function loadReadyAdds() {
    let setup;
    try {
      setup = await pa("add");
    } catch (e) {
      return;
    }
    const ready = setup.ready || [];
    const hashes = new Set(ready.map((r) => r.hash));
    for (const h of [...shownAdds]) if (!hashes.has(h)) shownAdds.delete(h);
    for (let i = addQueue.length - 1; i >= 0; i--) if (addQueue[i].hash && !hashes.has(addQueue[i].hash)) addQueue.splice(i, 1);
    if (addState && addState.file.hash && !hashes.has(addState.file.hash)) {
      addState.done = true; // OK or Cancel in another window
      closeAddDialog();
    }
    for (const r of ready) {
      if (shownAdds.has(r.hash)) continue;
      shownAdds.add(r.hash);
      addQueue.push({ hash: r.hash, name: r.name });
    }
    if (!addDlg.open) nextAdd();
  }
  function queueTorrents(files) {
    for (const f of files) addQueue.push(f);
    if (!addDlg.open) nextAdd();
  }
  async function nextAdd() {
    const file = addQueue.shift();
    if (!file) return;
    if (file.hash) {
      let info;
      let setup = { save_in: [""], labels, start: true };
      try {
        info = await pa("torrent-info?hash=" + encodeURIComponent(file.hash));
        setup = await pa("add");
      } catch (e) {
        if (!info) {
          nextAdd();
          return;
        }
      }
      setup.in_session = true;
      openAddDialog(file, info, setup);
      return;
    }
    const fd = new FormData();
    fd.append("torrent", file, file.name);
    let info;
    try {
      info = await pa("torrent-info", fd);
    } catch (e) {
      nextAdd();
      return;
    }
    let setup = { save_in: [""], labels, start: true, show_dialog: true };
    try {
      setup = await pa("add");
    } catch (e) {
      /* the defaults */
    }
    if (setup.show_dialog === false) {
      // the server's preference: no dialog, the defaults
      const out = new FormData();
      out.append("torrent", file, file.name);
      out.append("options", JSON.stringify({}));
      pa("add", out).catch(() => {});
      nextAdd();
      return;
    }
    openAddDialog(file, info, setup);
  }

  // drag-and-drop of .torrent files and magnet text, and a pasted magnet
  document.addEventListener("dragover", (ev) => {
    ev.preventDefault();
    ev.dataTransfer.dropEffect = "copy";
  });
  document.addEventListener("drop", (ev) => {
    ev.preventDefault();
    const files = [...(ev.dataTransfer.files || [])].filter((f) => /\.torrent$/i.test(f.name) || f.type === "application/x-bittorrent");
    if (files.length) {
      queueTorrents(files);
      return;
    }
    const text = (ev.dataTransfer.getData("text/uri-list") || ev.dataTransfer.getData("text/plain") || "").trim();
    if (/^magnet:/i.test(text) || /^https?:\/\//i.test(text)) addUrl(text);
  });
  document.addEventListener("paste", (ev) => {
    const t = ev.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    const text = (ev.clipboardData.getData("text/plain") || "").trim();
    if (/^magnet:/i.test(text)) {
      ev.preventDefault();
      addUrl(text);
    }
  });

  // the file tree: folders before the files of their parent, as the torrent lists them
  function buildTree(files) {
    const nodes = [];
    const roots = [];
    const byFile = [];
    const dirs = new Map(); // parent index + "/" + name -> node index
    for (const f of files) {
      const parts = f.path.split("/").filter((x) => x);
      let parent = -1;
      for (let d = 0; d < parts.length - 1; d++) {
        const key = parent + "/" + parts[d];
        let n = dirs.get(key);
        if (n === undefined) {
          n = nodes.length;
          nodes.push({ name: parts[d], depth: d, parent, dir: true, children: [], expanded: true });
          (parent < 0 ? roots : nodes[parent].children).push(n);
          dirs.set(key, n);
        }
        parent = n;
      }
      const n = nodes.length;
      nodes.push({ name: parts[parts.length - 1] || f.path, depth: parts.length - 1, parent, dir: false, i: f.i, size: f.size, wanted: true, prio: 4 });
      (parent < 0 ? roots : nodes[parent].children).push(n);
      byFile.push(n);
    }
    return { nodes, roots, byFile };
  }
  /// Folder summaries: files, wanted, size, the one priority of the wanted files (or null).
  function summarize(tree) {
    const { nodes } = tree;
    for (let n = nodes.length - 1; n >= 0; n--) {
      const node = nodes[n];
      if (node.dir) {
        let files = 0;
        let wanted = 0;
        let size = 0;
        let prio = null;
        let mixed = false;
        for (const c of node.children) {
          const ch = nodes[c];
          files += ch.dir ? ch.files : 1;
          const w = ch.dir ? ch.wanted : ch.wanted ? 1 : 0;
          wanted += w;
          size += ch.dir ? ch.size : ch.size;
          if (w > 0) {
            const p = ch.dir ? ch.prio : ch.prio;
            if (ch.dir && ch.mixed) mixed = true;
            else if (prio === null) prio = p;
            else if (prio !== p) mixed = true;
          }
        }
        node.files = files;
        node.wanted = wanted;
        node.size = size;
        node.prio = prio;
        node.mixed = mixed;
      }
    }
  }
  function filesOf(tree, n, out) {
    const node = tree.nodes[n];
    if (node.dir) for (const c of node.children) filesOf(tree, c, out);
    else out.push(node);
    return out;
  }
  function visibleRows(tree) {
    const rows = [];
    const walk = (n) => {
      rows.push(n);
      const node = tree.nodes[n];
      if (node.dir && node.expanded) for (const c of node.children) walk(c);
    };
    for (const r of tree.roots) walk(r);
    return rows;
  }

  function openAddDialog(file, info, setup) {
    const tree = buildTree(info.files || []);
    summarize(tree);
    addState = { file, info, tree, sel: new Set(), anchor: -1 };
    const dirs = $("add-dirs");
    dirs.textContent = "";
    for (const d of setup.save_in || []) {
      const o = document.createElement("option");
      o.value = d;
      dirs.appendChild(o);
    }
    $("add-save").value = (setup.save_in || [""])[0] || "";
    const ls = $("add-labels");
    ls.textContent = "";
    for (const l of setup.labels || labels) {
      const o = document.createElement("option");
      o.value = l;
      ls.appendChild(o);
    }
    $("add-label").value = "";
    $("add-name").value = info.name || "";
    $("add-skip").checked = false;
    // a waiting magnet's files were checked when its metadata arrived
    $("add-skip").disabled = !!setup.in_session;
    $("add-start").checked = setup.start !== false;
    $("add-top").checked = false;
    setText("add-t-name", info.name || "");
    setText("add-t-comment", info.comment || "");
    setText("add-t-date", info.date_text || "");
    renderAddFiles();
    addDlg.showModal();
    layerOpened(closeAddDialog);
    $("add-ok").focus();
  }
  function closeAddDialog() {
    // Cancel (the button, Esc, Back) on a waiting magnet removes it
    if (addState && addState.file.hash && !addState.done) pa("add", { hash: addState.file.hash, cancel: true }).catch(() => {});
    if (addDlg.open) addDlg.close();
    addState = null;
    $("add-files").textContent = "";
    nextAdd();
  }
  /// The rows: a check box, the name indented by depth with a toggle for folders, the size,
  /// the priority; folders show Mixed when their files differ. Rebuilt whole: the dialog is
  /// modal and the tree small compared with the list.
  function renderAddFiles() {
    const st = addState;
    if (!st) return;
    const { tree } = st;
    const el = $("add-files");
    el.textContent = "";
    const table = document.createElement("table");
    table.className = "dt af";
    const thead = document.createElement("thead");
    const hr = document.createElement("tr");
    for (const [title, cls] of [["", "c"], [T("ColName"), ""], [T("ColSize"), "r"], [T("ColPriority"), ""]]) {
      const th = document.createElement("th");
      th.textContent = title;
      if (cls) th.className = cls;
      hr.appendChild(th);
    }
    thead.appendChild(hr);
    const tbody = document.createElement("tbody");
    for (const n of visibleRows(tree)) {
      const node = tree.nodes[n];
      const tr = document.createElement("tr");
      tr.dataset.n = n;
      if (st.sel.has(n)) tr.className = "sel";
      const tdc = document.createElement("td");
      tdc.className = "c";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.tabIndex = -1;
      if (node.dir) {
        cb.checked = node.wanted > 0 && node.wanted === node.files;
        cb.indeterminate = node.wanted > 0 && node.wanted < node.files;
      } else cb.checked = node.wanted;
      tdc.appendChild(cb);
      const tdn = document.createElement("td");
      tdn.className = "n";
      tdn.style.paddingLeft = 6 + node.depth * 16 + "px";
      if (node.dir) {
        const tg = document.createElement("span");
        tg.className = "tg";
        tg.textContent = node.expanded ? "▾" : "▸";
        tdn.appendChild(tg);
      } else {
        const sp = document.createElement("span");
        sp.className = "tg";
        tdn.appendChild(sp);
      }
      tdn.appendChild(document.createTextNode(node.name));
      tdn.title = node.name;
      const tds = document.createElement("td");
      tds.className = "r";
      tds.textContent = fmtSize(node.size);
      const tdp = document.createElement("td");
      tdp.textContent = node.dir ? (node.wanted === 0 ? T("PrioDontDownload") : node.mixed || node.wanted < node.files ? T("AddMixed") : priorityText(node.prio)) : priorityText(node.wanted ? node.prio : 0);
      tr.append(tdc, tdn, tds, tdp);
      tbody.appendChild(tr);
    }
    table.append(thead, tbody);
    el.appendChild(table);
    updateAddSize();
  }
  function updateAddSize() {
    const { tree, info } = addState;
    let sel = 0;
    for (const node of tree.nodes) if (!node.dir && node.wanted) sel += node.size;
    const all = info.size || 0;
    setText("add-t-size", sel === all ? fmtSize(all) : T("AddSizeSelected", { selected: fmtSize(sel), total: fmtSize(all) }));
  }
  function setChecks(nodesIdx, on) {
    const { tree } = addState;
    for (const n of nodesIdx) for (const f of filesOf(tree, n, [])) f.wanted = on;
    summarize(tree);
    renderAddFiles();
  }
  function setPriority(nodesIdx, p) {
    const { tree } = addState;
    for (const n of nodesIdx) {
      for (const f of filesOf(tree, n, [])) {
        if (p === 0) f.wanted = false;
        else {
          f.wanted = true;
          f.prio = p;
        }
      }
    }
    summarize(tree);
    renderAddFiles();
  }
  $("add-files").addEventListener("click", (ev) => {
    const tr = ev.target.closest("tr[data-n]");
    if (!tr || !addState) return;
    const n = Number(tr.dataset.n);
    const node = addState.tree.nodes[n];
    if (ev.target.tagName === "INPUT") {
      // the check box: a row fully on turns off, anything else turns on; with the row
      // selected among others, all of them follow
      const rows = addState.sel.has(n) ? [...addState.sel] : [n];
      const on = node.dir ? !(node.wanted === node.files) : !node.wanted;
      setChecks(rows, on);
      return;
    }
    if (ev.target.classList.contains("tg") && node.dir) {
      node.expanded = !node.expanded;
      renderAddFiles();
      return;
    }
    const st = addState;
    if (ev.ctrlKey || ev.metaKey) {
      if (st.sel.has(n)) st.sel.delete(n);
      else st.sel.add(n);
      st.anchor = n;
    } else if (ev.shiftKey && st.anchor >= 0) {
      const rows = visibleRows(st.tree);
      const a = rows.indexOf(st.anchor);
      const b = rows.indexOf(n);
      st.sel = new Set(rows.slice(Math.min(a, b), Math.max(a, b) + 1));
    } else {
      st.sel = new Set([n]);
      st.anchor = n;
    }
    for (const r of $("add-files").querySelectorAll("tr[data-n]")) r.classList.toggle("sel", st.sel.has(Number(r.dataset.n)));
  });
  $("add-files").addEventListener("dblclick", (ev) => {
    const tr = ev.target.closest("tr[data-n]");
    if (!tr || !addState || ev.target.tagName === "INPUT") return;
    const node = addState.tree.nodes[Number(tr.dataset.n)];
    if (node.dir) {
      node.expanded = !node.expanded;
      renderAddFiles();
    }
  });
  $("add-files").addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    const tr = ev.target.closest("tr[data-n]");
    if (!tr || !addState) return;
    const n = Number(tr.dataset.n);
    const st = addState;
    if (!st.sel.has(n)) {
      st.sel = new Set([n]);
      st.anchor = n;
      for (const r of $("add-files").querySelectorAll("tr[data-n]")) r.classList.toggle("sel", st.sel.has(Number(r.dataset.n)));
    }
    const rows = [...st.sel];
    openMenu(
      [
        ...PRIORITIES.map(([key, p]) => ({ label: T(key), run: () => setPriority(rows, p) })),
        "-",
        { label: T("MenuCheck"), run: () => setChecks(rows, true) },
        { label: T("MenuUncheck"), run: () => setChecks(rows, false) },
      ],
      ev.clientX,
      ev.clientY
    );
  });
  $("add-all").addEventListener("click", () => addState && setChecks(addState.tree.roots, true));
  $("add-none").addEventListener("click", () => addState && setChecks(addState.tree.roots, false));
  $("add-cancel").addEventListener("click", closeAddDialog);
  addDlg.addEventListener("close", () => layerClosed(closeAddDialog));
  addDlg.addEventListener("cancel", (ev) => {
    ev.preventDefault();
    closeAddDialog();
  });
  $("add-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const st = addState;
    if (!st) return;
    const save = $("add-save").value.trim();
    if (!save) {
      $("add-save").focus();
      flash(T("AddNoFolder"));
      return;
    }
    const options = {
      save,
      label: $("add-label").value.trim(),
      start: $("add-start").checked,
      skip_check: $("add-skip").checked,
      top: $("add-top").checked,
      priorities: {},
    };
    const name = $("add-name").value.trim();
    if (name && name !== st.info.name) options.name = name;
    for (const node of st.tree.nodes) {
      if (node.dir) continue;
      const p = node.wanted ? node.prio : 0;
      if (p !== 4) options.priorities[node.i] = p;
    }
    try {
      if (st.file.hash) {
        await pa("add", Object.assign({ hash: st.file.hash }, options));
      } else {
        const fd = new FormData();
        fd.append("torrent", st.file, st.file.name);
        fd.append("options", JSON.stringify(options));
        await pa("add", fd);
      }
    } catch (e) {
      return; // shown; the dialog stays so the choice is not lost
    }
    st.done = true;
    if (options.label && !labels.includes(options.label)) {
      // a new label: the tree learns it now rather than at the next hello
      labels.push(options.label);
      labels.sort();
      renderTree();
    }
    closeAddDialog();
  });

  // ---- the RSS Downloader (µTorrent 2.2.1's dialog; pa/rss, nox's webui/rss.rs) -----------
  // Feeds on the left (All Feeds, then each feed: Add, Edit, Remove, Update), three tabs on the
  // right: the selected feed's Items (the Win32 window lists them in the torrent list; a
  // double-click or Download adds one), Filters (the list with check boxes, the selected
  // filter's fields, the items it matches) and History. Edits go to the server as they are
  // made, as in the Win32 dialog; Close runs the filters over the feeds' items. The tick's
  // `rss` version says when to reload; the category tree lists the feeds too.
  const rssDlg = $("rss");
  const feedDlg = $("rss-feed");
  let rssModel = null;
  let rssFeed = 0; // the feed picked on the left, 0 = All Feeds
  let rssFilter = 0; // the filter whose fields show
  let rssTab = store.get("pa.rssTab", "filters");
  let rssItems = []; // the Items tab's rows
  let rssItemSel = -1;
  let rssEditTimer = 0;
  let rssPending = {}; // filter fields typed but not sent yet
  let rssFeedEdit = 0; // the feed the Add RSS Feed dialog edits, 0 = a new one
  let rssVersion = 0;

  const rssEl = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  };
  const feedName = (id) => (rssModel.feeds.find((f) => f.id === id) || {}).name || "";

  /// The tick's version: the tree's Feeds branch and an open dialog follow it.
  function rssChanged(v) {
    if (v === rssVersion) return;
    rssVersion = v;
    rssLoad();
  }
  async function rssLoad() {
    try {
      rssModel = await pa("rss");
    } catch (e) {
      return;
    }
    rssRenderTree();
    if (!rssDlg.open) return;
    if (rssFeed && !rssModel.feeds.some((f) => f.id === rssFeed)) rssFeed = 0;
    rssRenderFeeds();
    rssRenderFilters();
    rssRenderHistory();
    if (rssTab === "items") rssLoadItems();
  }

  // the category tree's Feeds branch: a click opens the dialog on the feed's items
  function rssRenderTree() {
    const tree = $("tree");
    for (const e of tree.querySelectorAll(".rss-t")) e.remove();
    if (!rssModel || !rssModel.feeds.length || (hello && hello.hosted)) return;
    tree.appendChild(rssEl("div", "group rss-t", T("CatFeeds")));
    for (const f of rssModel.feeds) {
      const it = rssEl("div", "fitem rss-t");
      it.dataset.feed = f.id;
      const t = rssEl("span", "t", f.name);
      t.title = f.status;
      it.append(t, rssEl("span", "n", String(f.items)));
      tree.appendChild(it);
    }
  }
  $("tree").addEventListener("click", (ev) => {
    const it = ev.target.closest(".fitem");
    if (!it) return;
    setDrawer(false);
    openRss(+it.dataset.feed, "items");
  });

  async function openRss(feed, tab) {
    if (rssDlg.open || $("app").hidden || (hello && hello.hosted)) return;
    rssFeed = feed || 0;
    if (tab) rssTab = tab;
    rssDlg.showModal();
    layerOpened(closeRss);
    rssShowTab(rssTab);
    await rssLoad();
  }
  function closeRss() {
    if (!rssDlg.open) return;
    rssFlush();
    rssDlg.close();
    // the filters may have changed: they run over the feeds' current items
    pa("rss/process", {}).catch(() => {});
    body.focus();
  }
  rssDlg.addEventListener("close", () => layerClosed(closeRss));
  rssDlg.addEventListener("cancel", (ev) => {
    ev.preventDefault();
    if (!feedDlg.open) closeRss();
  });
  $("rss-close").addEventListener("click", closeRss);
  $("tb-rss").addEventListener("click", () => openRss(0));

  function rssShowTab(tab) {
    rssTab = tab;
    store.set("pa.rssTab", tab);
    for (const b of $("rss-tabs").children) b.classList.toggle("on", b.dataset.tab === tab);
    for (const p of rssDlg.querySelectorAll(".rss-page")) p.hidden = p.dataset.tab !== tab;
    if (tab === "items" && rssModel) rssLoadItems();
  }
  $("rss-tabs").addEventListener("click", (ev) => {
    const b = ev.target.closest("button");
    if (b) rssShowTab(b.dataset.tab);
  });

  // a list of rows, one picked; `check` adds the check boxes of the filter list
  function rssList(el, rows, picked, check) {
    el.textContent = "";
    rows.forEach(([key, text, on, tip]) => {
      const r = rssEl("div", "rss-row" + (key === picked ? " on" : ""));
      r.dataset.key = key;
      if (tip) r.title = tip;
      if (check) {
        const c = rssEl("input");
        c.type = "checkbox";
        c.checked = on;
        r.appendChild(c);
      }
      r.appendChild(rssEl("span", "", text));
      el.appendChild(r);
    });
  }

  // ---- Feeds
  function rssRenderFeeds() {
    const rows = [[0, T("RssAllFeeds")], ...rssModel.feeds.map((f) => [f.id, f.enabled ? f.name : T("WebFeedDisabled", { name: f.name }), false, f.status])];
    rssList($("rss-feed-list"), rows, rssFeed);
    const f = rssModel.feeds.find((x) => x.id === rssFeed);
    const n = rssModel.feeds.length;
    $("rss-feed-status").textContent = f ? f.status : T("RssFeedCount", { count: n });
    $("rss-feed-edit").disabled = $("rss-feed-remove").disabled = !f;
  }
  $("rss-feed-list").addEventListener("click", (ev) => {
    const r = ev.target.closest(".rss-row");
    if (!r) return;
    rssFeed = +r.dataset.key;
    rssRenderFeeds();
    if (rssTab === "items") rssLoadItems();
  });
  $("rss-feed-list").addEventListener("dblclick", () => rssFeed && rssFeedDialog(rssFeed));
  $("rss-feed-add").addEventListener("click", () => rssFeedDialog(0));
  $("rss-feed-edit").addEventListener("click", () => rssFeedDialog(rssFeed));
  $("rss-feed-update").addEventListener("click", () => pa("rss/feed-update", rssFeed ? { id: rssFeed } : {}).catch(() => {}));
  $("rss-feed-remove").addEventListener("click", async () => {
    if (!rssFeed || !confirm(T("AskDeleteFeed", { name: feedName(rssFeed) }))) return;
    await pa("rss/feed-remove", { id: rssFeed }).catch(() => {});
    rssFeed = 0;
    rssLoad();
  });

  // the Add RSS Feed dialog (also Edit)
  function rssFeedDialog(id) {
    const f = rssModel.feeds.find((x) => x.id === id);
    rssFeedEdit = f ? id : 0;
    $("rss-feed-title").textContent = f ? T("RssEditFeed") : T("RssAddFeed");
    $("rss-f-url").value = f ? f.url : "";
    $("rss-f-alias").value = f ? f.alias : "";
    $(f && f.subscribe ? "rss-f-auto" : "rss-f-noauto").checked = true;
    $("rss-f-smart").checked = !!(f && f.smart);
    $("rss-f-smart").disabled = !(f && f.subscribe);
    $("rss-f-msg").textContent = "";
    feedDlg.showModal();
    $("rss-f-url").focus();
  }
  $("rss-feed-form").addEventListener("change", () => ($("rss-f-smart").disabled = !$("rss-f-auto").checked));
  $("rss-f-cancel").addEventListener("click", () => feedDlg.close());
  $("rss-feed-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const url = $("rss-f-url").value.trim();
    if (!url) {
      $("rss-f-msg").textContent = T("RssNoUrl");
      return;
    }
    const subscribe = $("rss-f-auto").checked;
    const b = { url, alias: $("rss-f-alias").value.trim(), subscribe, smart: subscribe && $("rss-f-smart").checked };
    try {
      if (rssFeedEdit) await pa("rss/feed-edit", { id: rssFeedEdit, ...b });
      else rssFeed = (await pa("rss/feed-add", b)).id;
    } catch (e) {
      $("rss-f-msg").textContent = e.msg || T("WebFeedNotSaved");
      return;
    }
    feedDlg.close();
    rssLoad();
  });

  // ---- Items
  async function rssLoadItems() {
    let m;
    try {
      m = await pa("rss/items" + (rssFeed ? "?feed=" + rssFeed : ""));
    } catch (e) {
      return;
    }
    rssItems = m.rows;
    rssItemSel = -1;
    const t = $("rss-items");
    t.textContent = "";
    const head = rssEl("tr");
    for (const c of m.columns) {
      const th = rssEl("th", "", c.title);
      th.style.width = c.width + "px";
      head.appendChild(th);
    }
    t.appendChild(rssEl("thead")).appendChild(head);
    const tb = t.appendChild(rssEl("tbody"));
    rssItems.slice(0, 2000).forEach((r, i) => {
      const tr = rssEl("tr", r.downloaded ? "done" : "");
      tr.dataset.i = i;
      for (const c of r.cells) tr.appendChild(rssEl("td", "", c));
      tb.appendChild(tr);
    });
    $("rss-item-dl").disabled = true;
  }
  function rssItemPick(tr) {
    for (const x of $("rss-items").querySelectorAll("tr.on")) x.classList.remove("on");
    tr.classList.add("on");
    rssItemSel = +tr.dataset.i;
    $("rss-item-dl").disabled = !rssItems[rssItemSel].can;
  }
  async function rssDownload() {
    const r = rssItems[rssItemSel];
    if (!r || !r.can) return;
    await pa("rss/download", { feed: r.feed, item: r.id }).catch(() => {});
    rssLoadItems();
  }
  $("rss-items").addEventListener("click", (ev) => {
    const tr = ev.target.closest("tbody tr");
    if (tr) rssItemPick(tr);
  });
  $("rss-items").addEventListener("dblclick", (ev) => {
    const tr = ev.target.closest("tbody tr");
    if (!tr) return;
    rssItemPick(tr);
    rssDownload();
  });
  $("rss-item-dl").addEventListener("click", rssDownload);

  // ---- Filters
  const RSS_TEXT = ["name", "pattern", "not", "save_in", "episodes", "label"];
  const RSS_CHECK = ["smart", "top_priority", "paused"];
  function rssRenderFilters() {
    const fs = rssModel.filters;
    if (!fs.some((f) => f.id === rssFilter)) rssFilter = fs.length ? fs[0].id : 0;
    rssList($("rss-filter-list"), fs.map((f) => [f.id, f.name, f.enabled]), rssFilter, true);
    // the feed and interval choices, the labels
    const feedSel = $("rss-x-feed");
    feedSel.textContent = "";
    for (const [id, name] of [[0, T("RssAllFeeds")], ...rssModel.feeds.map((f) => [f.id, f.name])]) feedSel.appendChild(new Option(name, id));
    const iv = $("rss-x-min_interval");
    if (!iv.options.length) for (const [s, name] of rssModel.intervals) iv.appendChild(new Option(name, s));
    const dl = $("rss-labels");
    dl.textContent = "";
    for (const l of rssModel.labels) dl.appendChild(new Option(l));
    rssShowFilter();
  }
  function rssShowFilter() {
    const f = rssModel.filters.find((x) => x.id === rssFilter);
    const fields = $("rss-fields");
    for (const e of fields.querySelectorAll("input, select, button")) e.disabled = !f;
    for (const id of ["rss-filter-remove", "rss-filter-up", "rss-filter-down"]) $(id).disabled = !f;
    const active = document.activeElement;
    for (const k of RSS_TEXT) {
      const e = $("rss-x-" + k);
      // a field being typed in keeps what is typed
      if (e !== active || !f) e.value = f ? f[k] : "";
    }
    for (const k of RSS_CHECK) $("rss-x-" + k).checked = !!(f && f[k]);
    $("rss-x-feed").value = f ? f.feed : 0;
    $("rss-x-min_interval").value = f ? f.min_interval : 0;
    $("rss-x-quality").textContent = f ? f.quality_text : T("RssQualityAll");
    rssPreview();
  }
  async function rssPreview() {
    const el = $("rss-preview");
    if (!rssFilter) {
      el.textContent = "";
      return;
    }
    let rows;
    try {
      rows = await pa("rss/preview?filter=" + rssFilter);
    } catch (e) {
      return;
    }
    rssList(el, rows.map(([title, feed], i) => [i, title, false, feed]), -1);
  }
  /// Sends the typed fields (after a pause, or at once before something else).
  async function rssFlush() {
    clearTimeout(rssEditTimer);
    rssEditTimer = 0;
    if (!rssFilter || !Object.keys(rssPending).length) return;
    const b = { id: rssFilter, ...rssPending };
    rssPending = {};
    await pa("rss/filter-edit", b).catch(() => {});
  }
  function rssEdit(k, v, now) {
    rssPending[k] = v;
    clearTimeout(rssEditTimer);
    rssEditTimer = setTimeout(async () => {
      await rssFlush();
      rssPreview();
    }, now ? 0 : 400);
    if (k === "name") {
      const row = $("rss-filter-list").querySelector(".rss-row.on span");
      if (row) row.textContent = v;
    }
  }
  $("rss-fields").addEventListener("input", (ev) => {
    const k = ev.target.id.replace("rss-x-", "");
    if (RSS_TEXT.includes(k)) rssEdit(k, ev.target.value);
  });
  $("rss-fields").addEventListener("change", (ev) => {
    const k = ev.target.id.replace("rss-x-", "");
    if (RSS_CHECK.includes(k)) rssEdit(k, ev.target.checked, true);
    else if (k === "feed" || k === "min_interval") rssEdit(k, +ev.target.value, true);
  });
  $("rss-x-quality").addEventListener("click", (ev) => {
    const f = rssModel.filters.find((x) => x.id === rssFilter);
    if (!f) return;
    const set = async (q) => {
      f.quality = q;
      await rssFlush();
      await pa("rss/filter-edit", { id: f.id, quality: q }).catch(() => {});
      rssLoad();
    };
    const r = ev.currentTarget.getBoundingClientRect();
    openMenu(
      [
        { label: T("RssQualityAll"), chk: f.quality === 0, run: () => set(0) },
        "-",
        ...rssModel.qualities.map(([bit, name]) => ({ label: name, chk: (f.quality & bit) !== 0, run: () => set(f.quality ^ bit) })),
      ],
      RTL ? r.right : r.left,
      r.bottom
    );
  });
  $("rss-filter-list").addEventListener("click", async (ev) => {
    const r = ev.target.closest(".rss-row");
    if (!r) return;
    const id = +r.dataset.key;
    await rssFlush();
    if (ev.target.type === "checkbox") {
      await pa("rss/filter-edit", { id, enabled: ev.target.checked }).catch(() => {});
      return;
    }
    rssFilter = id;
    for (const x of $("rss-filter-list").children) x.classList.toggle("on", x === r);
    rssShowFilter();
  });
  $("rss-filter-add").addEventListener("click", async () => {
    await rssFlush();
    let m;
    try {
      // bound to the feed picked on the left, as the Win32 dialog does
      m = await pa("rss/filter-add", { name: T("RssNewFilter"), feed: rssFeed });
    } catch (e) {
      return;
    }
    rssFilter = m.id;
    await rssLoad();
    $("rss-x-name").select();
  });
  for (const [id, cmd] of [["rss-filter-remove", "filter-remove"], ["rss-filter-up", "filter-up"], ["rss-filter-down", "filter-down"]]) {
    $(id).addEventListener("click", async () => {
      if (!rssFilter) return;
      await rssFlush();
      const fs = rssModel.filters;
      const at = fs.findIndex((f) => f.id === rssFilter);
      await pa("rss/" + cmd, { id: rssFilter }).catch(() => {});
      if (cmd === "filter-remove") rssFilter = (fs[at + 1] || fs[at - 1] || {}).id || 0;
      rssLoad();
    });
  }
  $("rss-browse").addEventListener("click", () => {
    const v = prompt(T("WebRssSaveIn"), $("rss-x-save_in").value);
    if (v === null) return;
    $("rss-x-save_in").value = v.trim();
    rssEdit("save_in", v.trim(), true);
  });

  // ---- History
  function rssRenderHistory() {
    const t = $("rss-history");
    t.textContent = "";
    const head = rssEl("tr");
    for (const h of ["ColName", "ColDate", "ColFeed"]) head.appendChild(rssEl("th", "", T(h)));
    t.appendChild(rssEl("thead")).appendChild(head);
    const tb = t.appendChild(rssEl("tbody"));
    for (const row of rssModel.history) {
      const tr = rssEl("tr");
      for (const c of row) tr.appendChild(rssEl("td", "", c));
      tb.appendChild(tr);
    }
  }
  $("rss-history-clear").addEventListener("click", async () => {
    await pa("rss/history-clear", {}).catch(() => {});
    rssLoad();
  });

  // ---- Preferences (docs/WEBUI.md slice 6) -------------------------------------------------
  // The dialog is generated from GET pa/prefs: `pages` (the 2.2.1 tree, each page its groups of
  // items), `fields` (id -> label, type, value, default, range, options) and `advanced` (every
  // engine setting's id). A field needs no code here, only a place on a page. Edits are kept per
  // field; Apply and OK send only the changed ones (POST pa/prefs), Cancel discards them.
  const prefsDlg = $("prefs");
  let prefsModel = null;
  let prefsEdits = new Map(); // id -> the value to send
  let prefsRanges = new Map(); // id -> [min, max, scale] of the control that made the edit
  let prefsPageId = store.get("pa.prefsPage", "general");
  let advFilter = "";
  let advSel = null; // the selected Advanced row's id
  let adv = null; // the Advanced page's elements while it is shown
  let confirmDelete = true;

  function prefField(id) {
    return prefsModel.fields[id];
  }
  function prefValue(id) {
    return prefsEdits.has(id) ? prefsEdits.get(id) : prefField(id).value;
  }
  function prefSet(id, v) {
    const f = prefField(id);
    if (f.type === "password" ? v === "" : v === f.value) prefsEdits.delete(id);
    else prefsEdits.set(id, v);
  }
  function prefIsDefault(id) {
    return prefValue(id) === prefField(id).default;
  }
  function setPrefsMsg(text, bad) {
    const el = $("prefs-msg");
    el.textContent = text;
    el.classList.toggle("bad", !!bad);
  }

  async function openPrefs() {
    if (prefsDlg.open || $("app").hidden) return;
    let m;
    try {
      m = await pa("prefs");
    } catch (e) {
      return;
    }
    prefsModel = m;
    prefsEdits = new Map();
    prefsRanges = new Map();
    advSel = null;
    advFilter = "";
    setPrefsMsg("");
    buildPrefsTree();
    showPrefsPage(prefsPageId);
    prefsDlg.showModal();
    layerOpened(closePrefs);
    $("prefs-tree").querySelector(".on").focus();
  }
  function closePrefs() {
    if (prefsDlg.open) prefsDlg.close();
    prefsEdits = new Map();
    adv = null;
    body.focus();
  }

  // the tree
  function buildPrefsTree() {
    const tree = $("prefs-tree");
    tree.textContent = "";
    for (const p of prefsModel.pages) {
      const it = document.createElement("div");
      it.className = "pt" + (p.under ? " sub" : "");
      it.setAttribute("role", "treeitem");
      it.tabIndex = -1;
      it.dataset.page = p.id;
      it.textContent = p.title;
      tree.appendChild(it);
    }
  }
  function markPrefsTree() {
    for (const it of $("prefs-tree").children) {
      const on = it.dataset.page === prefsPageId;
      it.classList.toggle("on", on);
      it.tabIndex = on ? 0 : -1;
      it.setAttribute("aria-selected", on);
    }
  }
  $("prefs-tree").addEventListener("click", (ev) => {
    const it = ev.target.closest(".pt");
    if (it) showPrefsPage(it.dataset.page);
  });
  $("prefs-tree").addEventListener("keydown", (ev) => {
    const pages = prefsModel.pages;
    const i = pages.findIndex((p) => p.id === prefsPageId);
    let j = i;
    if (ev.key === "ArrowDown") j = Math.min(pages.length - 1, i + 1);
    else if (ev.key === "ArrowUp") j = Math.max(0, i - 1);
    else if (ev.key === "Home") j = 0;
    else if (ev.key === "End") j = pages.length - 1;
    else return;
    ev.preventDefault();
    showPrefsPage(pages[j].id);
    $("prefs-tree").children[j].focus();
  });

  // a page, from its groups of items
  function showPrefsPage(id) {
    const page = prefsModel.pages.find((p) => p.id === id) || prefsModel.pages[0];
    prefsPageId = page.id;
    store.set("pa.prefsPage", page.id);
    markPrefsTree();
    const el = $("prefs-page");
    el.textContent = "";
    el.classList.toggle("adv", !!page.advanced);
    adv = null;
    if (page.advanced) renderAdvanced(el);
    for (const g of page.groups) {
      const box = document.createElement(g.title ? "fieldset" : "div");
      box.className = "pg";
      if (g.title) {
        const lg = document.createElement("legend");
        lg.textContent = g.title;
        box.appendChild(lg);
      }
      for (const it of g.items) box.appendChild(renderItem(it));
      el.appendChild(box);
    }
    updateEnabled(el);
  }
  function renderItem(it) {
    if (it.row) {
      const r = document.createElement("div");
      r.className = "pf-row";
      for (const sub of it.row) r.appendChild(renderItem(sub));
      return r;
    }
    if (it.note !== undefined) {
      const p = document.createElement("p");
      p.className = "pf-note";
      p.textContent = it.note;
      return p;
    }
    if (it.button) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = it.button;
      b.dataset.action = it.action;
      return b;
    }
    if (it.f && prefField(it.f).type === "schedule") return renderSchedule(it);
    const l = document.createElement("label");
    if (it.stub !== undefined) {
      // a placeholder until its feature exists, disabled as in the Win32 dialog
      const inp = document.createElement("input");
      inp.disabled = true;
      if (it.text !== undefined) {
        l.className = "pf txt";
        const sp = document.createElement("span");
        sp.textContent = it.stub;
        l.appendChild(sp);
        inp.type = "text";
        inp.value = it.text;
        l.appendChild(inp);
      } else {
        l.className = "pf chk";
        inp.type = "checkbox";
        inp.checked = !!it.on;
        l.appendChild(inp);
        l.appendChild(document.createTextNode(" " + it.stub));
      }
      return l;
    }
    const f = prefField(it.f);
    const v = prefValue(it.f);
    let inp;
    if (f.type === "bool") {
      l.className = "pf chk";
      inp = document.createElement("input");
      inp.type = "checkbox";
      inp.checked = !!v;
      l.appendChild(inp);
      l.appendChild(document.createTextNode(" " + f.label));
    } else {
      l.className = "pf " + (f.type === "int" ? "num" : f.type === "enum" || f.type === "choice" ? "sel" : "txt");
      if (f.label) {
        const sp = document.createElement("span");
        sp.textContent = f.label;
        l.appendChild(sp);
      }
      if (f.type === "enum") {
        inp = document.createElement("select");
        f.options.forEach((o, i) => {
          const op = document.createElement("option");
          op.value = i;
          op.textContent = o;
          inp.appendChild(op);
        });
        inp.value = v;
      } else if (f.type === "choice") {
        // options are [value, text]; the value is sent as it is (the language's code)
        inp = document.createElement("select");
        for (const [value, text] of f.options) inp.appendChild(new Option(text, value));
        inp.value = v;
      } else {
        inp = document.createElement("input");
        if (f.type === "int") {
          // kB/s on the page, bytes on the wire; a value that is not a whole kB shows rounded and
          // is kept unless the field is edited
          const scale = it.kb ? 1024 : 1;
          const min = it.min !== undefined ? it.min : f.min;
          const max = it.max !== undefined ? it.max : f.max;
          inp.type = "number";
          inp.step = 1;
          inp.min = Math.ceil(min / scale);
          inp.max = Math.floor(max / scale);
          inp.dataset.min = min;
          inp.dataset.max = max;
          inp.dataset.scale = scale;
          inp.value = Math.trunc(v / scale);
        } else {
          inp.type = f.type === "password" || it.secret ? "password" : "text";
          inp.value = v;
          inp.autocomplete = f.type === "password" ? "new-password" : "off";
          if (f.type === "password") inp.placeholder = T("WebUnchanged");
        }
        if (f.readonly) inp.readOnly = true;
      }
      l.appendChild(inp);
    }
    inp.dataset.id = it.f;
    if (it.enabled_by) l.dataset.enabledBy = it.enabled_by;
    return l;
  }
  /// A control's value as the server takes it; an int that is not a whole number comes back as
  /// the text, for the range check to refuse.
  function readControl(inp) {
    const f = prefField(inp.dataset.id);
    if (f.type === "bool") return inp.checked;
    if (f.type === "enum") return Number(inp.value);
    if (f.type === "int") {
      const t = inp.value.trim();
      if (!/^-?\d+$/.test(t)) return t;
      return Number(t) * Number(inp.dataset.scale || 1);
    }
    return inp.value;
  }
  $("prefs-page").addEventListener("input", (ev) => {
    const inp = ev.target;
    if (!inp.dataset.id) return;
    inp.classList.remove("bad");
    prefSet(inp.dataset.id, readControl(inp));
    if (inp.dataset.min !== undefined) {
      prefsRanges.set(inp.dataset.id, [Number(inp.dataset.min), Number(inp.dataset.max), Number(inp.dataset.scale)]);
    }
    updateEnabled($("prefs-page"));
  });
  /// "Move completed downloads to" and the like gate the controls under them, as in µTorrent
  /// (several space-separated ids: all must be on).
  function updateEnabled(root) {
    for (const l of root.querySelectorAll("[data-enabled-by]")) {
      const on = l.dataset.enabledBy.split(" ").every((id) => !!prefValue(id));
      for (const c of l.querySelectorAll("input,select,button")) c.disabled = !on;
      l.classList.toggle("disabled", !on);
    }
  }

  // The Scheduler table (frontend::scheduler): 7 days x 24 hours, one mode code per hour,
  // Monday 00:00 first. A click (or tap) moves a cell to the next mode, a drag paints that mode.
  const SCHED_DAYS = ["DayMon", "DayTue", "DayWed", "DayThu", "DayFri", "DaySat", "DaySun"].map((k) => T(k));
  const SCHED_MODES = { 0: T("SchedFullSpeed"), 2: T("SchedLimited"), 1: T("SchedTurnOff"), 3: T("SchedSeedingOnly") };
  const SCHED_NEXT = { 0: "2", 2: "1", 1: "3", 3: "0" };
  function renderSchedule(it) {
    const id = it.f;
    const box = document.createElement("div");
    box.className = "sched";
    if (it.enabled_by) box.dataset.enabledBy = it.enabled_by;
    const grid = document.createElement("div");
    grid.className = "sched-grid";
    grid.appendChild(document.createElement("span"));
    for (let h = 0; h < 24; h++) {
      const s = document.createElement("span");
      s.className = "sched-h";
      s.textContent = h % 3 === 0 ? h : "";
      grid.appendChild(s);
    }
    const cells = [];
    const table = () => String(prefValue(id));
    for (let d = 0; d < 7; d++) {
      const s = document.createElement("span");
      s.className = "sched-d";
      s.textContent = SCHED_DAYS[d];
      grid.appendChild(s);
      for (let h = 0; h < 24; h++) {
        const c = document.createElement("span");
        c.className = "sched-c";
        c.dataset.i = d * 24 + h;
        grid.appendChild(c);
        cells.push(c);
      }
    }
    const info = document.createElement("div");
    info.className = "sched-info";
    const legend = document.createElement("div");
    legend.className = "sched-legend";
    for (const code of ["0", "2", "1", "3"]) {
      const s = document.createElement("span");
      const sw = document.createElement("i");
      sw.className = "m" + code;
      s.appendChild(sw);
      s.appendChild(document.createTextNode(SCHED_MODES[code]));
      legend.appendChild(s);
    }
    const paint = () => {
      const t = table();
      cells.forEach((c, i) => (c.className = "sched-c m" + t[i]));
    };
    const describe = (i) => {
      const t = table();
      const d = Math.floor(i / 24), h = i % 24;
      const two = (n) => String(n).padStart(2, "0");
      info.textContent = T("SchedHover", { day: SCHED_DAYS[d], from: two(h), to: two((h + 1) % 24), mode: SCHED_MODES[t[i]] });
    };
    const setCell = (i, code) => {
      const t = table();
      if (t[i] === code) return;
      prefSet(id, t.slice(0, i) + code + t.slice(i + 1));
      paint();
    };
    let painting = null;
    let last = null; // the cell the drag last painted
    // every cell on the line from `last` to `i`, so a fast drag leaves no gaps
    const paintTo = (i) => {
      const d0 = Math.floor(last / 24), h0 = last % 24, d1 = Math.floor(i / 24), h1 = i % 24;
      const n = Math.max(Math.abs(d1 - d0), Math.abs(h1 - h0));
      for (let k = 1; k <= n; k++) {
        setCell(Math.round(d0 + ((d1 - d0) * k) / n) * 24 + Math.round(h0 + ((h1 - h0) * k) / n), painting);
      }
      last = i;
    };
    const cellAt = (ev) => {
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      return el && el.classList.contains("sched-c") && grid.contains(el) ? Number(el.dataset.i) : null;
    };
    grid.addEventListener("pointerdown", (ev) => {
      if (box.classList.contains("disabled") || ev.button !== 0) return;
      const i = cellAt(ev);
      if (i === null) return;
      ev.preventDefault();
      painting = SCHED_NEXT[table()[i]];
      last = i;
      setCell(i, painting);
      describe(i);
      grid.setPointerCapture(ev.pointerId);
    });
    grid.addEventListener("pointermove", (ev) => {
      const i = cellAt(ev);
      if (i === null) return;
      if (painting !== null) paintTo(i);
      describe(i);
    });
    const stop = () => (painting = null);
    grid.addEventListener("pointerup", stop);
    grid.addEventListener("pointercancel", stop);
    grid.addEventListener("pointerleave", () => painting === null && (info.textContent = ""));
    paint();
    box.appendChild(grid);
    box.appendChild(legend);
    box.appendChild(info);
    return box;
  }
  $("prefs-page").addEventListener("click", (ev) => {
    const b = ev.target.closest("button[data-action]");
    if (!b) return;
    if (b.dataset.action === "random_port") {
      // frontend::options::random_port's range
      const inp = $("prefs-page").querySelector('[data-id="listen_port"]');
      if (!inp) return;
      inp.value = 10000 + Math.floor(Math.random() * 55001);
      inp.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });

  // the Advanced page: the full settings table, a filter, a value editor, Set / Reset
  function renderAdvanced(el) {
    const bar = document.createElement("div");
    bar.className = "adv-bar";
    const lab = document.createElement("label");
    lab.className = "pf txt";
    const sp = document.createElement("span");
    sp.textContent = T("PrefFilter");
    const filt = document.createElement("input");
    filt.type = "search";
    filt.id = "adv-filter";
    filt.autocomplete = "off";
    filt.value = advFilter;
    lab.appendChild(sp);
    lab.appendChild(filt);
    bar.appendChild(lab);
    el.appendChild(bar);

    const list = document.createElement("div");
    list.className = "adv-list";
    const table = document.createElement("table");
    table.className = "adv";
    const thead = document.createElement("thead");
    const hr = document.createElement("tr");
    for (const key of ["ColName", "ColValue"]) {
      const th = document.createElement("th");
      th.textContent = T(key);
      hr.appendChild(th);
    }
    thead.appendChild(hr);
    table.appendChild(thead);
    const tbody = document.createElement("tbody");
    table.appendChild(tbody);
    list.appendChild(table);
    el.appendChild(list);

    const ed = document.createElement("div");
    ed.className = "adv-edit";
    const vl = document.createElement("span");
    vl.textContent = T("PrefValue");
    const value = document.createElement("input");
    value.type = "text";
    value.id = "adv-value";
    value.autocomplete = "off";
    const radios = document.createElement("span");
    radios.className = "radios";
    const mk = (text, val) => {
      const l = document.createElement("label");
      const r = document.createElement("input");
      r.type = "radio";
      r.name = "adv-bool";
      r.value = val;
      l.appendChild(r);
      l.appendChild(document.createTextNode(" " + text));
      radios.appendChild(l);
      return r;
    };
    const rTrue = mk(T("PrefTrue"), "true");
    const rFalse = mk(T("PrefFalse"), "false");
    const set = document.createElement("button");
    set.type = "button";
    set.textContent = T("PrefSet");
    const reset = document.createElement("button");
    reset.type = "button";
    reset.textContent = T("PrefReset");
    ed.append(vl, value, radios, set, reset);
    el.appendChild(ed);

    adv = { tbody, value, radios, rTrue, rFalse, set, reset };
    filt.addEventListener("input", () => {
      advFilter = filt.value.trim().toLowerCase();
      fillAdvanced();
    });
    tbody.addEventListener("click", (ev) => {
      const tr = ev.target.closest("tr[data-id]");
      if (!tr) return;
      advSel = tr.dataset.id;
      for (const r of tbody.children) r.classList.toggle("sel", r.dataset.id === advSel);
      showAdvEditor();
    });
    tbody.addEventListener("dblclick", (ev) => {
      if (ev.target.closest("tr[data-id]")) adv.value.hidden ? adv.rTrue.focus() : adv.value.focus();
    });
    set.addEventListener("click", () => advSet(false));
    reset.addEventListener("click", () => advSet(true));
    fillAdvanced();
  }
  /// The rows matching the filter, sorted by name, changed values in bold (2.2.1's `*`).
  function fillAdvanced() {
    if (!adv) return;
    const { tbody } = adv;
    tbody.textContent = "";
    const ids = prefsModel.advanced.filter((id) => id.includes(advFilter)).sort();
    for (const id of ids) {
      const tr = document.createElement("tr");
      tr.dataset.id = id;
      if (!prefIsDefault(id)) tr.classList.add("changed");
      if (id === advSel) tr.classList.add("sel");
      const n = document.createElement("td");
      n.className = "n";
      n.textContent = id;
      const v = document.createElement("td");
      v.textContent = String(prefValue(id));
      tr.append(n, v);
      tbody.appendChild(tr);
    }
    if (advSel && !ids.includes(advSel)) advSel = null;
    showAdvEditor();
  }
  /// The value field for the selected row: True / False for switches, a text field otherwise.
  function showAdvEditor() {
    if (!adv) return;
    const f = advSel ? prefField(advSel) : null;
    const isBool = !!f && f.type === "bool";
    adv.value.hidden = isBool;
    adv.radios.hidden = !isBool;
    adv.value.classList.remove("bad");
    if (f) {
      const v = prefValue(advSel);
      if (isBool) {
        adv.rTrue.checked = v === true;
        adv.rFalse.checked = v !== true;
      } else adv.value.value = String(v);
    } else adv.value.value = "";
    for (const c of [adv.value, adv.rTrue, adv.rFalse, adv.set, adv.reset]) c.disabled = !f;
  }
  /// "Set" (or "Reset" to the default) for the selected row.
  function advSet(reset) {
    if (!adv || !advSel) return;
    const f = prefField(advSel);
    let v;
    if (reset) v = f.default;
    else if (f.type === "bool") v = adv.rTrue.checked;
    else {
      const t = adv.value.value;
      if (f.type === "int") {
        if (!/^-?\d+$/.test(t.trim()) || Number(t) < f.min || Number(t) > f.max) {
          adv.value.classList.add("bad");
          adv.value.focus();
          setPrefsMsg(advSel + ": please enter a number from " + f.min + " to " + f.max + ".", true);
          return;
        }
        v = Number(t);
      } else v = t;
    }
    prefSet(advSel, v);
    prefsRanges.set(advSel, [f.min, f.max, 1]);
    setPrefsMsg("");
    fillAdvanced();
  }

  // Apply / OK / Cancel and the errors
  /// The page of a field, for an error: the one that shows it, else Advanced.
  function pageOfPref(id) {
    const has = (items) => items.some((it) => it.f === id || (it.row && has(it.row)));
    const p = prefsModel.pages.find((p) => p.groups.some((g) => has(g.items)));
    if (p) return p.id;
    return prefsModel.advanced.includes(id) ? prefsModel.pages.find((p) => p.advanced).id : null;
  }
  function showPrefError(id, msg) {
    const pg = pageOfPref(id);
    if (pg) showPrefsPage(pg);
    const inp = $("prefs-page").querySelector('[data-id="' + id + '"]');
    if (inp) {
      inp.classList.add("bad");
      inp.focus();
    } else if (adv && prefsModel.advanced.includes(id)) {
      advSel = id;
      fillAdvanced();
      adv.value.classList.add("bad");
      const tr = adv.tbody.querySelector(".sel");
      if (tr) tr.scrollIntoView({ block: "nearest" });
    }
    setPrefsMsg(msg, true);
  }
  /// The Win32 dialog's range check on the fields edited here, before anything is sent.
  function validatePrefs() {
    for (const [id, v] of prefsEdits) {
      const f = prefField(id);
      if (f.type !== "int") continue;
      const [min, max, scale] = prefsRanges.get(id) || [f.min, f.max, 1];
      if (typeof v !== "number" || !Number.isInteger(v) || v < min || v > max) {
        return [id, T("PrefBadNumber", { min: Math.ceil(min / scale), max: Math.floor(max / scale) })];
      }
    }
    return null;
  }
  async function applyPrefs(close) {
    const bad = validatePrefs();
    if (bad) {
      showPrefError(bad[0], bad[1]);
      return;
    }
    if (prefsEdits.size) {
      try {
        await pa("prefs", Object.fromEntries(prefsEdits));
      } catch (e) {
        // {"error": "<field>: <why>"} names the first bad field
        const m = /^([^\s:]+): ([\s\S]*)$/.exec(e.msg || "");
        if (m && prefField(m[1])) showPrefError(m[1], e.msg);
        else setPrefsMsg(e.msg || T("WebPrefsNotApplied"), true);
        return;
      }
      if (prefsEdits.has("ui.confirm_delete")) confirmDelete = !!prefsEdits.get("ui.confirm_delete");
      // a new language: the server's texts follow at once, the page's with a reload (lang.js)
      if (prefsEdits.has("ui.language")) {
        location.reload();
        return;
      }
      prefsEdits = new Map();
      if (close) {
        closePrefs();
        return;
      }
      // the composites (the port in listen_interfaces, the proxy type) read back exactly
      try {
        prefsModel = await pa("prefs");
      } catch (e) {
        /* the dialog keeps what it has */
      }
      showPrefsPage(prefsPageId);
      setPrefsMsg(T("WebApplied"));
      return;
    }
    if (close) closePrefs();
  }
  $("prefs-form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    applyPrefs(true);
  });
  $("prefs-form").addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter") return;
    if (ev.ctrlKey || ev.metaKey) {
      ev.preventDefault();
      applyPrefs(true);
      return;
    }
    const t = ev.target;
    if (t.id === "adv-value") {
      // Enter in the Advanced value field sets the value instead of closing
      ev.preventDefault();
      advSet(false);
    } else if (t.tagName === "INPUT" || t.tagName === "SELECT") {
      ev.preventDefault(); // Enter in a field applies nothing, as 2.2.1
    }
  });
  $("prefs-apply").addEventListener("click", () => applyPrefs(false));
  $("prefs-cancel").addEventListener("click", closePrefs);
  prefsDlg.addEventListener("close", () => layerClosed(closePrefs));
  prefsDlg.addEventListener("cancel", (ev) => {
    ev.preventDefault(); // Escape
    closePrefs();
  });
  $("tb-prefs").addEventListener("click", openPrefs);
  document.addEventListener("keydown", (ev) => {
    if (!(ev.ctrlKey || ev.metaKey) || ev.shiftKey || ev.altKey || ev.key.toLowerCase() !== "p") return;
    if ($("app").hidden || addDlg.open || $("props").open) return;
    ev.preventDefault();
    if (!prefsDlg.open) openPrefs();
  });

  // ---- Torrent Properties (frontend::properties) -------------------------------------------
  // The Win32 dialog's General tab on GET / POST pa/torrents/properties, opened from the
  // context menu and by double-clicking a torrent that is not finished. The fields come as text
  // (blank where the selected torrents differ), the check boxes as "on", "off" or "mixed". OK
  // sends only the fields that changed: what the others hold stays as it is on every torrent.
  const propsDlg = $("props");
  let props = null; // {sel, loaded, multi, canSeed} while the dialog is open
  let propsBusy = false; // a GET or the OK's POST in flight

  const propsInputs = () => propsDlg.querySelectorAll("[data-f]");
  const propsField = (f) => propsDlg.querySelector('[data-f="' + f + '"]');
  function triOf(inp) {
    return inp.indeterminate ? "mixed" : inp.checked ? "on" : "off";
  }
  function setTri(inp, v) {
    inp.checked = v === "on";
    inp.indeterminate = v === "mixed";
    inp._tri = v;
  }
  /// The selection as the GET's query: the hashes, or with Ctrl+A the view's filter.
  function propsQuery(s) {
    if (!s.all) return "ids=" + s.ids.join(",");
    const q = new URLSearchParams({ all: "1", category: s.filter.category, search: s.filter.search });
    if (s.filter.label != null) q.set("label", s.filter.label);
    return q.toString();
  }
  async function openProps(dblclick) {
    const s = selection();
    if (!s || propsBusy || propsDlg.open || $("app").hidden) return;
    propsBusy = true;
    let m;
    try {
      m = await pa("torrents/properties?" + propsQuery(s));
    } catch (e) {
      return; // shown by pa()
    } finally {
      propsBusy = false;
    }
    // the Win32 window opens a finished torrent's folder instead, which a browser cannot
    if (dblclick && m.finished) return;
    props = { sel: s, loaded: m.fields, multi: m.count > 1, canSeed: m.can_initial_seed };
    $("props-title").textContent =
      m.count === 1 && m.name != null ? T("PropsTitle", { name: m.name }) : T("PropsTitleMany", { count: m.count });
    for (const inp of propsInputs()) {
      const v = m.fields[inp.dataset.f];
      if (inp.type === "checkbox") setTri(inp, v);
      else inp.value = v;
    }
    showPropsError(null, "");
    propsGrey();
    propsDlg.showModal();
    layerOpened(closeProps);
    const tr = $("props-trackers");
    tr.scrollTop = 0;
    if (phone) $("props-ok").focus(); // no keyboard popping up over the dialog
    else {
      tr.focus();
      tr.setSelectionRange(0, 0);
    }
  }
  function closeProps() {
    if (propsDlg.open) propsDlg.close();
    props = null;
    body.focus();
  }
  /// The slot count and the goal's values are edited only while their override is not off,
  /// Initial Seeding only when every torrent is a seed (the Win32 dialog's greying).
  function propsGrey() {
    const on = (f) => triOf(propsField(f)) !== "off";
    const grey = (f, off) => {
      propsField(f).disabled = off;
      propsField(f).closest(".pf").classList.toggle("disabled", off);
    };
    grey("upload_slots", !on("override_slots"));
    grey("goal_ratio", !on("override_goal"));
    grey("goal_minutes", !on("override_goal"));
    grey("initial_seeding", !props.canSeed);
  }
  /// A validation error under its field, which takes the focus with the text selected (the
  /// tracker box selects the line named); without a field, above the buttons.
  function showPropsError(inp, text) {
    const el = $("props-err");
    for (const i of propsInputs()) i.classList.remove("bad");
    el.textContent = text;
    el.hidden = !text;
    if (!text) return;
    if (!inp) {
      propsDlg.querySelector(".props-btns").before(el);
      return;
    }
    (inp.closest(".pf") || inp).after(el);
    inp.classList.add("bad");
    inp.focus();
    const line = /^"(.*)" is not/.exec(text);
    const at = line ? inp.value.indexOf(line[1]) : -1;
    if (at >= 0) inp.setSelectionRange(at, at + line[1].length);
    else inp.select();
  }
  async function okProps() {
    if (!props || propsBusy) return;
    const fields = {};
    for (const inp of propsInputs()) {
      const v = inp.type === "checkbox" ? triOf(inp) : inp.value;
      if (v !== props.loaded[inp.dataset.f]) fields[inp.dataset.f] = v;
    }
    if (Object.keys(fields).length) {
      propsBusy = true;
      try {
        await pa("torrents/properties", { ...props.sel, fields });
      } catch (e) {
        // {"error": "<field>: <why>"} names the first bad field
        const m = /^([a-z_]+): ([\s\S]*)$/.exec(e.msg || "");
        if (m && propsField(m[1])) showPropsError(propsField(m[1]), m[2]);
        else showPropsError(null, e.msg || T("WebPropsNotSet"));
        return;
      } finally {
        propsBusy = false;
      }
    }
    closeProps();
  }
  propsDlg.addEventListener("click", (ev) => {
    const inp = ev.target;
    if (inp.type !== "checkbox" || !inp.dataset.f) return;
    // several torrents give the boxes a third state, cycled as BS_AUTO3STATE: off, on, mixed
    const next = { off: "on", on: props && props.multi ? "mixed" : "off", mixed: "off" };
    setTri(inp, next[inp._tri] || "on");
    propsGrey();
  });
  propsDlg.addEventListener("input", (ev) => {
    if (ev.target.classList.contains("bad")) showPropsError(null, "");
  });
  $("props-form").addEventListener("submit", (ev) => {
    ev.preventDefault(); // Enter in a number field: OK, the Win32 dialog's default button
    okProps();
  });
  $("props-form").addEventListener("keydown", (ev) => {
    // Enter is a new line in the tracker box; Ctrl+Enter is OK anywhere, Enter on a check box too
    if (ev.key !== "Enter" || !(ev.ctrlKey || ev.metaKey || ev.target.type === "checkbox")) return;
    ev.preventDefault();
    okProps();
  });
  $("props-cancel").addEventListener("click", closeProps);
  propsDlg.addEventListener("close", () => layerClosed(closeProps));
  propsDlg.addEventListener("cancel", (ev) => {
    ev.preventDefault(); // Escape
    closeProps();
  });
  body.addEventListener("dblclick", (ev) => {
    if (!phone && rowOf(ev.target)) openProps(true);
  });

  // ---- splitters -----------------------------------------------------------------------
  const saved = store.get("pa.layout", {});
  if (saved.tree) document.documentElement.style.setProperty("--tree-w", saved.tree + "px");
  if (saved.detail) document.documentElement.style.setProperty("--detail-h", saved.detail + "px");
  document.documentElement.style.setProperty("--row-h", ROW_H + "px");
  document.documentElement.style.setProperty("--phone-row-h", PHONE_ROW_H + "px");
  function splitter(el, vertical) {
    el.addEventListener("pointerdown", (ev) => {
      ev.preventDefault();
      el.setPointerCapture(ev.pointerId);
      el.classList.add("drag");
      document.body.classList.add(vertical ? "drag-v" : "drag-h");
      const main = $("main").getBoundingClientRect();
      const right = $("right").getBoundingClientRect();
      const move = (e) => {
        if (vertical) {
          const w = Math.max(0, Math.min(main.width - 120, RTL ? main.right - e.clientX : e.clientX - main.left));
          saved.tree = Math.round(w);
          document.documentElement.style.setProperty("--tree-w", saved.tree + "px");
        } else {
          const h = Math.max(0, Math.min(right.height - 60, right.bottom - e.clientY));
          saved.detail = Math.round(h);
          document.documentElement.style.setProperty("--detail-h", saved.detail + "px");
        }
      };
      const up = () => {
        el.removeEventListener("pointermove", move);
        el.removeEventListener("pointerup", up);
        el.removeEventListener("pointercancel", up);
        el.classList.remove("drag");
        document.body.classList.remove("drag-v", "drag-h");
        store.set("pa.layout", saved);
      };
      el.addEventListener("pointermove", move);
      el.addEventListener("pointerup", up);
      el.addEventListener("pointercancel", up);
    });
  }
  splitter($("split-v"), true);
  splitter($("split-h"), false);

  // ---- visibility: an idle client costs the server nothing -------------------------------
  document.addEventListener("visibilitychange", () => {
    hidden = document.hidden;
    if (hidden) {
      gate.hiddenSince = performance.now();
      send({ t: "idle" });
    } else {
      if (gate.hiddenSince) gate.hiddenSecs += (performance.now() - gate.hiddenSince) / 1000;
      gate.hiddenSince = 0;
      sentWindow = null;
      sendView();
      gateRender();
    }
  });

  // ---- login ---------------------------------------------------------------------------
  function showLogin() {
    closing = true;
    clearTimeout(reconnectTimer);
    clearInterval(countdownTimer);
    if (ws) ws.close();
    ws = null;
    $("app").hidden = true;
    $("login").hidden = false;
    $("login").elements.username.focus();
  }
  function showApp() {
    $("login").hidden = true;
    $("app").hidden = false;
    body.focus();
    connect();
  }
  $("login").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    gate.tLogin = performance.now();
    const f = new FormData(ev.target);
    const r = await fetch("api/v2/auth/login", { method: "POST", body: new URLSearchParams(f), credentials: "same-origin" });
    const text = await r.text();
    if (r.ok && text === "Ok.") {
      stage("login");
      $("login-error").textContent = "";
      ev.target.elements.password.value = "";
      showApp();
    } else $("login-error").textContent = r.status === 403 ? text : T("WebWrongLogin");
  });
  $("logout").addEventListener("click", () => {
    api("auth/logout", {}).then(showLogin, showLogin);
  });

  fetch("api/v2/app/version", { credentials: "same-origin" }).then(
    (r) => (r.ok ? showApp() : showLogin()),
    () => showLogin()
  );
})();
