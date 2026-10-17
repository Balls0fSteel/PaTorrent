// PaTorrent's Web UI: the µTorrent 2.2.1 main window on the native session protocol
// (docs/WEBUI.md). The page holds only the visible window of rows; the server runs the
// filtered, sorted list and sends formatted cells. Commands still go to the compat shim
// (api/v2/...) until slice 5 brings the native command endpoints.
"use strict";
(() => {
  const ROW_H = 22; // must match --row-h in style.css (set below so they cannot drift)
  const OVERSCAN = 10;
  const MAX_WINDOW = 200;
  const LIMITS = [0, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000]; // kB/s, 0 = unlimited
  const GATE = location.hash === "#gate";
  const $ = (id) => document.getElementById(id);

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
  let detailTab = "general";
  let detailId = null;
  let labels = [];
  let pool = [];
  let scrollRaf = 0;
  let hidden = document.hidden;

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
      `first rows: ${gate.rowsBytes} B (${ids.length} rows, ${visibleColumns().length} cols)\n` +
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

  // ---- shim calls ----------------------------------------------------------------------
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
      for (const el of pool) buildCells(el, cols);
      sentWindow = null;
      sendView();
    }
    gateRender();
  }
  function buildCells(row, cols) {
    row.textContent = "";
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
    setConn("Connecting…");
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
        case "error":
          flash(m.msg || "error");
          break;
        default:
          break; // detail and log come with slice 4
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
    setConn("Disconnected, reconnecting in " + left + " s", true);
    clearInterval(countdownTimer);
    countdownTimer = setInterval(() => {
      left--;
      if (left > 0) setConn("Disconnected, reconnecting in " + left + " s", true);
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
    backoff = 1000;
    setConn("");
    labels = m.labels || [];
    mergeColumns(m.columns);
    saveLayout();
    renderTree();
    applyColumns(false);
    for (const el of pool) buildCells(el, visibleColumns());
    sentWindow = null;
    if (hidden) send({ t: "idle" });
    else sendView();
    if (detailId) send({ t: "detail", id: detailId, tab: detailTab });
  }

  /// The view subscription for what the list shows right now. `window` follows the scroll
  /// position; it is sent only when the visible rows are not covered by the last one.
  function sendView() {
    if (!hello) return;
    const [first, count] = neededWindow();
    sentWindow = [first, count];
    const f = { category: filter.label ? "all" : filter.category || "all", label: filter.label || null, search };
    send({ t: "view", filter: f, sort, columns: visibleColumns(), window: [first, count] });
  }
  function neededWindow() {
    const visFirst = Math.floor(body.scrollTop / ROW_H);
    const visCount = Math.ceil(body.clientHeight / ROW_H) + 1;
    const first = Math.max(0, visFirst - OVERSCAN);
    const count = Math.min(MAX_WINDOW, visCount + 2 * OVERSCAN);
    return [first, count];
  }
  function windowCovers(first, count) {
    if (!sentWindow) return false;
    const [sf, sc] = sentWindow;
    const visFirst = Math.floor(body.scrollTop / ROW_H);
    const visLast = visFirst + Math.ceil(body.clientHeight / ROW_H);
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
    spacer.style.height = total * ROW_H + "px";
    const empty = body.querySelector(".empty");
    if (total === 0 && hello) {
      if (!empty) {
        const e = document.createElement("div");
        e.className = "empty";
        e.textContent = search || filter.label || filter.category !== "all" ? "No torrents match." : "No torrents. Add one with the toolbar.";
        body.appendChild(e);
      }
    } else if (empty) empty.remove();
  }

  // ---- the virtual list ----------------------------------------------------------------
  function render() {
    const cols = visibleColumns();
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
      const top = index * ROW_H + "px";
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
    $("st-sel").textContent = n ? n + " selected" : "";
    const none = n === 0;
    for (const b of document.querySelectorAll("#toolbar [data-cmd]")) {
      if (b.dataset.cmd.startsWith("queue")) continue; // the shim has no queue moves yet
      b.disabled = none;
    }
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
    setDetail(id);
  }
  function ensureVisible(index) {
    const top = index * ROW_H;
    if (top < body.scrollTop) body.scrollTop = top;
    else if (top + ROW_H > body.scrollTop + body.clientHeight) body.scrollTop = top + ROW_H - body.clientHeight;
  }
  function setDetail(id) {
    if (id === detailId) return;
    detailId = id;
    send({ t: "detail", id, tab: detailTab });
  }
  function rowOf(target) {
    const el = target.closest(".row");
    return el && !el.hidden && el._id ? el : null;
  }
  body.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0 && ev.button !== 2) return;
    const el = rowOf(ev.target);
    if (!el) {
      if (ev.button === 0 && (ev.target === spacer || ev.target === body)) {
        sel = new Set();
        selAll = false;
        updateSelClasses();
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
    const el = rowOf(ev.target);
    if (!el && sel.size === 0 && !selAll) return;
    openTorrentMenu(ev.clientX, ev.clientY);
  });
  body.addEventListener("keydown", (ev) => {
    if (ev.target !== body) return;
    const page = Math.max(1, Math.floor(body.clientHeight / ROW_H) - 1);
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
    openTorrentMenu(r.left + 40, el ? r.bottom : r.top + 20);
  }

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
          after = e.clientX > (r.left + r.right) / 2;
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
    sort = sort && sort[0] === id ? [id, !sort[1]] : [id, !!c.desc];
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
      layout.widths[id] = Math.max(20, w0 + e.clientX - x0);
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
      label: "Reset Columns",
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
    g.textContent = "Labels";
    tree.appendChild(g);
    if (labels.length === 0) {
      const e = document.createElement("div");
      e.className = "empty";
      e.textContent = "No labels";
      tree.appendChild(e);
    }
    for (const l of labels) {
      const it = document.createElement("div");
      it.className = "item";
      it.dataset.label = l;
      const t = document.createElement("span");
      t.className = "t";
      t.textContent = l;
      t.title = l;
      it.appendChild(t);
      tree.appendChild(it);
    }
    markTree();
  }
  function markTree() {
    for (const it of $("tree").querySelectorAll(".item")) {
      const on = filter.label ? it.dataset.label === filter.label : it.dataset.cat === filter.category;
      it.classList.toggle("on", on);
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
    filter = it.dataset.label ? { category: "all", label: it.dataset.label } : { category: it.dataset.cat, label: null };
    store.set("pa.filter", filter);
    markTree();
    body.scrollTop = 0;
    sentWindow = null;
    sendView();
  });

  // ---- status bar ----------------------------------------------------------------------
  function updateStatus(s) {
    setText("st-dht", "DHT: " + s.dht + " nodes");
    setText("st-down", "D: " + s.down + " T: " + s.down_total);
    setText("st-up", "U: " + s.up + " T: " + s.up_total);
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

  // ---- commands (the compat shim, until slice 5) ----------------------------------------
  function unfiltered() {
    return !filter.label && (filter.category || "all") === "all" && !search;
  }
  /// The `hashes` parameter for the selection: "all" when everything in an unfiltered view
  /// is selected, else the selected ids (with Ctrl+A under a filter, the window's ids).
  function selectedHashes() {
    if (selAll && unfiltered()) return "all";
    const set = new Set(sel);
    if (selAll) for (const id of ids) set.add(id);
    return set.size ? [...set].join("|") : null;
  }
  function selectedCount() {
    return selAll ? total : sel.size;
  }
  function nameOfSelection() {
    if (selectedCount() === 1 && !selAll) {
      const id = [...sel][0];
      const c = cells.get(id);
      const nameAt = visibleColumns().indexOf("name");
      if (c && nameAt >= 0) return c.c[nameAt];
    }
    return selectedCount() + " torrents";
  }
  async function command(name, arg) {
    const hashes = selectedHashes();
    if (!hashes) return;
    const form = { hashes };
    let path;
    switch (name) {
      case "start":
        path = "torrents/start";
        break;
      case "force_start":
        path = "torrents/setForceStart";
        form.value = "true";
        break;
      case "pause":
        path = "torrents/pause";
        break;
      case "stop":
        path = "torrents/stop";
        break;
      case "remove":
      case "remove_data": {
        const data = name === "remove_data";
        const what = nameOfSelection();
        const msg = data
          ? "Remove " + what + " and delete the downloaded data?"
          : "Remove " + what + "? The downloaded files are kept.";
        if (!confirm(msg)) return;
        path = "torrents/delete";
        form.deleteFiles = data ? "true" : "false";
        if (hashes !== "all") for (const id of hashes.split("|")) sel.delete(id);
        if (hashes === "all") sel = new Set();
        selAll = false;
        cur = -1;
        detailId = null;
        send({ t: "detail", id: null });
        updateSelClasses();
        break;
      }
      case "recheck":
        path = "torrents/recheck";
        break;
      case "reannounce":
        path = "torrents/reannounce";
        break;
      case "superseed":
        path = "torrents/setSuperSeeding";
        form.value = arg ? "true" : "false";
        break;
      case "dl_limit":
        path = "torrents/setDownloadLimit";
        form.limit = String(arg * 1024);
        break;
      case "ul_limit":
        path = "torrents/setUploadLimit";
        form.limit = String(arg * 1024);
        break;
      case "label":
        path = "torrents/setCategory";
        form.category = arg || "";
        break;
      case "label_new": {
        const l = prompt("New label:");
        if (!l || !l.trim()) return;
        const name = l.trim();
        try {
          if (!labels.includes(name)) {
            await api("torrents/createCategory", { category: name, savePath: "" });
            labels.push(name);
            labels.sort();
            renderTree();
          }
          await api("torrents/setCategory", { hashes, category: name });
        } catch (e) {
          /* shown by api() */
        }
        return;
      }
      default:
        return;
    }
    try {
      await api(path, form);
    } catch (e) {
      /* shown by api() */
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
      { label: "Open Containing Folder", ...dis("not from a browser") },
      "-",
      { label: "Force Start", run: () => command("force_start") },
      { label: "Start", run: () => command("start") },
      { label: "Pause", run: () => command("pause") },
      { label: "Stop", run: () => command("stop") },
      "-",
      { label: "Move Up Queue", ...dis("slice 5") },
      { label: "Move Down Queue", ...dis("slice 5") },
      { label: "Move to Top of Queue", ...dis("slice 5") },
      { label: "Move to Bottom of Queue", ...dis("slice 5") },
      "-",
      { label: "Remove", run: () => command("remove") },
      { label: "Remove And", sub: [{ label: "Delete Data", run: () => command("remove_data") }] },
      "-",
      { label: "Force Re-Check", run: () => command("recheck") },
      { label: "Update Tracker", run: () => command("reannounce") },
      "-",
      {
        label: "Advanced",
        sub: [
          { label: "Set Download Location...", ...dis("slice 5") },
          { label: "Initial Seeding", key: "superseed", run: (it) => command("superseed", !it.chk) },
        ],
      },
      {
        label: "Bandwidth Allocation",
        sub: [
          { label: "Set Download Limit", sub: limitItems("dl") },
          { label: "Set Upload Limit", sub: limitItems("ul") },
        ],
      },
      {
        label: "Label",
        sub: [
          { label: "New Label...", run: () => command("label_new") },
          { label: "Remove Label", key: "label:", run: () => command("label", "") },
          ...(labels.length ? ["-"] : []),
          ...labels.map((l) => ({ label: l, key: "label:" + l, run: () => command("label", l) })),
        ],
      },
    ];
    openMenu(items, x, y);
    fillChecks(items);
  }
  function limitItems(kind) {
    return LIMITS.map((kb) => ({
      label: kb === 0 ? "Unlimited" : kb + " kB/s",
      key: kind + ":" + kb,
      run: () => command(kind + "_limit", kb),
    }));
  }
  /// Check marks need the torrents' current limits, label and super-seeding flag, which the
  /// cells do not carry: one shim call, applied while the menu is still open.
  async function fillChecks(items) {
    const hashes = selectedHashes();
    if (!hashes || hashes === "all" || hashes.split("|").length > 100) return;
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
  function openMenu(items, x, y) {
    closeMenu();
    const root = $("menu");
    buildMenu(root, items);
    showMenuAt(root, x, y);
    menus.push(root);
    setTimeout(() => {
      document.addEventListener("pointerdown", onDocPointer, true);
      document.addEventListener("keydown", onMenuKey, true);
    });
    window.addEventListener("blur", closeMenu, { once: true });
  }
  function buildMenu(el, items) {
    el.textContent = "";
    el.hidden = false;
    el.className = "menu";
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
      mi.className = "mi" + (it.dis ? " dis" : "") + (it.chk ? " chk" : "") + (it.sub ? " sub" : "");
      mi.setAttribute("role", "menuitem");
      mi.textContent = it.label;
      if (it.hint) mi.title = it.hint;
      it.el = mi;
      mi._item = it;
      mi.addEventListener("pointerenter", () => {
        highlight(el, mi);
        if (it.sub && !it.dis) openSub(el, mi, it.sub);
        else closeSubsOf(el);
      });
      mi.addEventListener("click", (ev) => {
        ev.stopPropagation();
        if (it.dis) return;
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
    document.body.appendChild(sub);
    buildMenu(sub, items);
    const r = mi.getBoundingClientRect();
    let x = r.right - 2;
    if (x + sub.offsetWidth > innerWidth) x = Math.max(0, r.left - sub.offsetWidth + 2);
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
    switch (ev.key) {
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

  // ---- detail tabs (content with slice 4) -----------------------------------------------
  $("tabs").addEventListener("click", (ev) => {
    const b = ev.target.closest("[data-tab]");
    if (!b) return;
    detailTab = b.dataset.tab;
    for (const t of $("tabs").children) t.classList.toggle("on", t === b);
    if (detailId) send({ t: "detail", id: detailId, tab: detailTab });
  });

  // ---- toolbar: add --------------------------------------------------------------------
  $("tb-add").addEventListener("click", () => $("file").click());
  $("file").addEventListener("change", async (ev) => {
    const files = [...ev.target.files];
    ev.target.value = "";
    if (!files.length) return;
    const fd = new FormData();
    for (const f of files) fd.append("torrents", f, f.name);
    try {
      const text = await api("torrents/add", fd);
      if (text !== "Ok.") flash("Add: " + text);
    } catch (e) {
      /* shown */
    }
  });
  $("tb-add-url").addEventListener("click", async () => {
    const url = prompt("Add Torrent from URL\n\nPlease enter the location of the torrent you want to open (a URL or a magnet link):");
    if (!url || !url.trim()) return;
    try {
      const text = await api("torrents/add", { urls: url.trim() });
      if (text !== "Ok.") flash("Add: " + text);
    } catch (e) {
      /* shown */
    }
  });

  // ---- splitters -----------------------------------------------------------------------
  const saved = store.get("pa.layout", {});
  if (saved.tree) document.documentElement.style.setProperty("--tree-w", saved.tree + "px");
  if (saved.detail) document.documentElement.style.setProperty("--detail-h", saved.detail + "px");
  document.documentElement.style.setProperty("--row-h", ROW_H + "px");
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
          const w = Math.max(0, Math.min(main.width - 120, e.clientX - main.left));
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
    } else $("login-error").textContent = r.status === 403 ? text : "Wrong user name or password.";
  });
  $("logout").addEventListener("click", () => {
    api("auth/logout", {}).then(showLogin, showLogin);
  });

  fetch("api/v2/app/version", { credentials: "same-origin" }).then(
    (r) => (r.ok ? showApp() : showLogin()),
    () => showLogin()
  );
})();
