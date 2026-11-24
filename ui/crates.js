function createAlbumCrates({ invoke, lookup, play, back, toast, coverUrl, blankColor }) {
  const el = id => document.getElementById(id);
  const page = el('cratesPage'), shelf = el('crateShelf'), tray = el('crateTray');
  const fan = el('crateFan'), fanCovers = el('fanCovers');
  const SPRING = 'cubic-bezier(.3,1.35,.5,1)';
  let data = { active: null, lists: [] };
  let pending = Promise.resolve();
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const listById = id => data.lists.find(c => c.id === id);

  function update(change) {
    const run = pending.then(async () => { data = await invoke('library_crates', { change }); return data; });
    pending = run.catch(e => { toast(String(e), 4500); });
    return run;
  }

  // Every record sits a little crooked, but always the same way.
  function tilt(id) {
    let h = 7;
    for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) % 9973;
    return (h % 9) - 4;
  }
  function cover(entry) {
    const a = lookup(entry.id) || entry;
    const c = document.createElement('div');
    c.className = 'cc';
    c.style.setProperty('--blank', blankColor(entry.id));
    const blank = () => {
      c.classList.add('blank');
      const t = document.createElement('span');
      t.textContent = a.title;
      c.appendChild(t);
    };
    if (a.art) {
      const img = new Image();
      img.decoding = 'async';
      img.draggable = false;
      img.src = coverUrl(a, 'thumb');
      img.onerror = () => { img.remove(); blank(); };
      c.appendChild(img);
    } else blank();
    return c;
  }
  function stack(list, size) {
    const s = document.createElement('div');
    s.className = 'stack';
    s.style.setProperty('--s', size + 'px');
    const top = list ? list.albums.slice(0, 4) : [];
    if (!top.length) s.classList.add('empty');
    top.forEach((entry, i) => {
      const c = cover(entry);
      c.style.setProperty('--i', i);
      c.style.setProperty('--r', (i ? tilt(entry.id) * 0.9 : tilt(entry.id) * 0.3) + 'deg');
      s.prepend(c);
    });
    return s;
  }
  function label(list) {
    const n = list.albums.length;
    return n === 1 ? '1 record' : n + ' records';
  }

  /* ---------------- tray: appears only while a cover is pulled down ---------------- */

  let ghost = null, drop = null;
  let trayLists, trayWidth;
  function renderTray() {
    if (trayLists === data.lists && trayWidth === window.innerWidth) return;
    trayLists = data.lists;
    trayWidth = window.innerWidth;
    const box = el('trayCrates');
    const count = data.lists.length + 1;
    const size = Math.round(clamp(window.innerWidth / count - 46, 52, 92));
    const items = data.lists.map(list => trayItem(list, list.name, size));
    items.push(trayItem(null, 'New crate', size));
    box.replaceChildren(...items);
  }
  function trayItem(list, name, size) {
    const item = document.createElement('div');
    item.className = 'tray-crate' + (list ? '' : ' new');
    item.dataset.id = list ? list.id : '';
    const n = document.createElement('div');
    n.className = 'tray-name';
    n.textContent = name;
    item.append(stack(list, size), n);
    return item;
  }
  function targetAt(x, y) {
    return document.elementFromPoint(x, y)?.closest('.tray-crate') || null;
  }
  function removeGhost(g) {
    g.flight?.cancel();
    g.el.remove();
    g.sleeve.classList.remove('filing');
  }
  // A scan or cover upgrade can replace the source element during the landing.
  function bindFilingSleeve(album, sleeve) {
    if (ghost?.el.isConnected && ghost.album.id === album.id) {
      ghost.sleeve = sleeve;
      sleeve.classList.add('filing');
    }
  }
  function begin(album, sleeve, x, y) {
    if (ghost) removeGhost(ghost);
    renderTray();
    tray.querySelectorAll('.landed, .reject, .over').forEach(item => item.classList.remove('landed', 'reject', 'over'));
    drop = null;
    tray.classList.add('open');
    const r = sleeve.getBoundingClientRect();
    const size = Math.min(r.width, r.height);
    const g = sleeve.cloneNode(true);
    g.classList.replace('sleeve', 'crate-ghost');
    g.removeAttribute('style');
    g.style.setProperty('--blank', blankColor(album.id));
    g.style.width = g.style.height = size + 'px';
    const img = g.querySelector('img');
    if (img) img.decoding = 'sync';
    const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
    ghost = { el: g, album, sleeve, size, home: { x: cx, y: cy }, grab: { x: x - cx, y: y - cy },
      startY: y, lastX: x, rot: 0, trayTop: window.innerHeight - tray.offsetHeight };
    place(x, y);
    sleeve.classList.add('filing');
    document.body.appendChild(g);
  }
  function place(x, y) {
    const p = clamp((y - ghost.startY) / Math.max(80, ghost.trayTop - ghost.startY), 0, 1);
    const scale = 1 - p * 0.62;
    ghost.rot = ghost.rot * 0.8 + clamp((x - ghost.lastX) * 0.9, -16, 16) * 0.2;
    ghost.lastX = x;
    const cx = x - ghost.grab.x * scale, cy = y - ghost.grab.y * scale;
    ghost.el.style.setProperty('--motion-transform', `translate3d(${cx - ghost.size / 2}px,${cy - ghost.size / 2}px,0) scale(${scale}) rotate(${ghost.rot.toFixed(2)}deg)`);
  }
  function drag(x, y) {
    if (!ghost) return;
    place(x, y);
    const t = targetAt(x, y);
    if (t !== drop) {
      drop?.classList.remove('over');
      t?.classList.add('over');
      drop = t;
    }
  }
  function flyGhost(g, x, y, scale, rot) {
    const transform = `translate3d(${x - g.size / 2}px,${y - g.size / 2}px,0) scale(${scale}) rotate(${rot}deg)`;
    g.flight = g.el.animate([{ '--motion-transform': g.el.style.getPropertyValue('--motion-transform') }, { '--motion-transform': transform }], {
      duration: matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 550,
      easing: SPRING, fill: 'forwards',
    });
    // Grabbing another sleeve interrupts this flight.
    return g.flight.finished.catch(() => {});
  }
  async function end(x, y, cancelled) {
    if (!ghost) return;
    const g = ghost, target = cancelled ? null : targetAt(x, y);
    drop?.classList.remove('over');
    drop = null;
    const id = target?.dataset.id ? Number(target.dataset.id) : null;
    const duplicate = id !== null && listById(id)?.albums.some(e => e.id === g.album.id);
    if (!target || duplicate) {
      if (duplicate) {
        target.classList.add('reject');
        toast('Already in ' + listById(id).name);
      } else tray.classList.remove('open');
      await flyGhost(g, g.home.x, g.home.y, 1, 0);
      if (ghost !== g) return;
      removeGhost(g);
      ghost = null;
      tray.classList.remove('open');
      return;
    }
    const s = target.querySelector('.stack').getBoundingClientRect();
    const [saved] = await Promise.allSettled([
      update({ type: 'add', id, album: g.album.id }),
      flyGhost(g, s.left + s.width / 2, s.top + s.height / 2, s.width / g.size, tilt(g.album.id) * 0.3),
    ]);
    if (ghost !== g) return;
    if (saved.status === 'fulfilled') {
      const list = id === null ? data.lists[data.lists.length - 1] : listById(id);
      const fresh = trayItem(list, list.name, parseFloat(target.querySelector('.stack').style.getPropertyValue('--s')));
      fresh.classList.add('landed');
      target.replaceWith(fresh);
    }
    removeGhost(g);
    await wait(650);
    if (ghost === g) {
      ghost = null;
      tray.classList.remove('open');
    }
  }

  /* ---------------- crates page ---------------- */

  function shelfSize() {
    return Math.round(clamp(Math.min(window.innerWidth / 5, window.innerHeight / 3.6), 130, 220));
  }
  function renderShelf(entering = false) {
    const size = shelfSize();
    const items = data.lists.map((list, i) => {
      const b = document.createElement('button');
      b.className = 'shelf-crate';
      b.dataset.id = list.id;
      b.setAttribute('role', 'listitem');
      b.style.setProperty('--d', i * 45 + 'ms');
      const name = document.createElement('div');
      name.className = 'shelf-name';
      name.textContent = list.name;
      const meta = document.createElement('div');
      meta.className = 'shelf-meta';
      meta.textContent = label(list);
      b.append(stack(list, size), name, meta);
      b.addEventListener('click', () => unfold(list.id));
      return b;
    });
    const add = document.createElement('button');
    add.className = 'shelf-crate new';
    add.style.setProperty('--d', items.length * 45 + 'ms');
    const s = stack(null, size);
    s.textContent = '+';
    const n = document.createElement('div');
    n.className = 'shelf-name';
    n.textContent = 'New crate';
    add.append(s, n);
    add.addEventListener('click', async () => {
      await update({ type: 'create' });
      renderShelf();
      const id = data.lists[data.lists.length - 1]?.id;
      if (id) unfold(id, true);
    });
    items.push(add);
    if (entering) items.forEach(item => item.classList.add('entering'));
    shelf.replaceChildren(...items);
    el('cratesHint').textContent = data.lists.length
      ? 'Click a crate to spread it out. Drag covers to change the order.'
      : 'Pull a cover down in the library to file it into a crate.';
  }

  function open() {
    page.classList.remove('hidden');
    page.classList.remove('leaving');
    const shown = JSON.stringify(data.lists);
    renderShelf(true);
    update(null).then(() => {
      if (isOpen() && !fanState && JSON.stringify(data.lists) !== shown) renderShelf();
    }).catch(() => {});
    el('cratesBack').focus({ preventScroll: true });
  }
  function close() {
    if (fanState) closeFan(true);
    page.classList.add('hidden');
  }
  function isOpen() { return !page.classList.contains('hidden'); }
  function escape() {
    if (!el('fanNameInput').classList.contains('hidden')) finishRename(false);
    else if (fanState) collapse();
    else back();
  }
  el('cratesBack').addEventListener('click', () => back());

  /* ---------------- the fan ---------------- */

  let fanState = null; // { id, order: [entry], els: Map(id → el), S, origin }
  let gesture = null;

  function fanLayout(n, S) {
    const W = window.innerWidth * 0.86;
    const step = n > 1 ? Math.min(S * 1.04, (W - S) / (n - 1)) : 0;
    const total = step * (n - 1);
    const x0 = window.innerWidth / 2 - total / 2 - S / 2;
    const cy = window.innerHeight * 0.4 - S / 2;
    const spread = Math.min(9, 2 + n * 0.8);
    const slots = [];
    for (let i = 0; i < n; i++) {
      const t = n > 1 ? (i / (n - 1)) * 2 - 1 : 0;
      slots.push({ x: x0 + i * step, y: cy + t * t * S * 0.1, r: t * spread });
    }
    return { slots, step, x0, cy };
  }
  function setT(c, x, y, r, s = 1) {
    c.style.setProperty('--motion-transform', `translate3d(${x.toFixed(1)}px,${y.toFixed(1)}px,0) rotate(${r.toFixed(2)}deg) scale(${s})`);
  }
  function layoutFan(skip, order = fanState.order) {
    const { slots } = fanLayout(order.length, fanState.S);
    order.forEach((entry, i) => {
      const c = fanState.els.get(entry.id);
      c.style.zIndex = String(10 + i);
      if (c === skip) return;
      setT(c, slots[i].x, slots[i].y, slots[i].r);
    });
    const last = slots[slots.length - 1] || { y: window.innerHeight * 0.4 - fanState.S / 2 };
    el('fanFoot').style.top = Math.max(...slots.map(s => s.y), last.y) + fanState.S + 46 + 'px';
  }
  function stackOrigin(id) {
    const node = shelf.querySelector(`.shelf-crate[data-id="${id}"] .stack`);
    return node ? { node, rect: node.getBoundingClientRect() } : null;
  }
  function originT(i, S) {
    const o = fanState.origin;
    if (!o) return { x: window.innerWidth / 2 - S / 2, y: window.innerHeight, r: 0, s: 0.5 };
    const w = o.rect.width, k = Math.min(i, 3);
    return { x: o.rect.left + w / 2 - S / 2 + k * w * 0.07, y: o.rect.top + w / 2 - S / 2 - k * w * 0.055, r: 0, s: w / S };
  }

  function unfold(id, rename = false) {
    const list = listById(id);
    if (!list || fanState) return;
    const S = Math.round(clamp(Math.min(window.innerHeight * 0.34, window.innerWidth * 0.2), 120, 260));
    fanState = { id, order: list.albums.slice(), els: new Map(), S, origin: stackOrigin(id) };
    fanState.origin?.node.classList.add('lifted-away');
    fanCovers.style.setProperty('--S', S + 'px');
    fanCovers.replaceChildren();
    fanState.order.forEach((entry, i) => {
      const c = cover(entry);
      c.classList.add('fan-cover');
      c.dataset.id = entry.id;
      c.title = (lookup(entry.id) || entry).artist + ' — ' + (lookup(entry.id) || entry).title + (lookup(entry.id) ? '' : ' (unavailable)');
      const o = originT(i, S);
      c.style.transition = 'none';
      if (i > 3) c.style.setProperty('--motion-opacity', '0');
      setT(c, o.x, o.y, o.r, o.s);
      c.addEventListener('pointerdown', grab);
      fanCovers.appendChild(c);
      fanState.els.set(entry.id, c);
    });
    el('fanName').textContent = list.name;
    el('fanMeta').textContent = list.albums.length ? label(list) : 'Empty — pull covers down in the library to fill it.';
    el('fanPlay').disabled = !list.albums.some(e => lookup(e.id));
    el('fanHint').textContent = list.albums.length > 1 ? 'Drag to reorder · drag down to take out' : list.albums.length ? 'Drag down to take out' : '';
    resetDelete();
    fan.classList.remove('hidden', 'closing');
    page.classList.add('fanned');
    void fanCovers.offsetWidth;
    fanState.order.forEach((entry, i) => {
      const c = fanState.els.get(entry.id);
      const delay = Math.min(i, 12) * 32;
      c.style.transition = `--motion-transform .75s ${SPRING} ${delay}ms, --motion-opacity .4s ease ${delay}ms, box-shadow .3s`;
      c.style.removeProperty('--motion-opacity');
    });
    layoutFan();
    if (rename) setTimeout(startRename, 300);
    else el('fanClose').focus({ preventScroll: true });
  }

  async function collapse() {
    if (!fanState || fanState.closing) return;
    finishRename(true);
    const st = fanState;
    st.closing = true;
    renderShelf();
    st.origin = stackOrigin(st.id);
    st.origin?.node.classList.add('lifted-away');
    const n = st.order.length;
    st.order.forEach((entry, i) => {
      const c = st.els.get(entry.id);
      const o = originT(i, st.S);
      const delay = Math.min(n - 1 - i, 12) * 18;
      c.style.transition = `--motion-transform .5s cubic-bezier(.5,0,.2,1) ${delay}ms, --motion-opacity .35s ease ${delay + 150}ms`;
      c.style.zIndex = String(100 - i);
      setT(c, o.x, o.y, o.r, o.s);
      if (i > 3) c.style.setProperty('--motion-opacity', '0');
    });
    fan.classList.add('closing');
    page.classList.remove('fanned');
    await finishFanMotion(st);
    if (fanState !== st) return;
    closeFan();
  }
  function finishFanMotion(st) {
    const moving = [el('fanFoot'), ...st.els.values()].flatMap(node => node.getAnimations());
    return Promise.allSettled(moving.map(animation => animation.finished));
  }
  function closeFan() {
    shelf.querySelectorAll('.lifted-away').forEach(n => n.classList.remove('lifted-away'));
    fan.classList.add('hidden');
    fan.classList.remove('closing');
    page.classList.remove('fanned');
    fanCovers.replaceChildren();
    fanState = null;
    gesture = null;
  }

  function grab(e) {
    if (e.button !== 0 || !fanState || fanState.closing || gesture) return;
    const c = e.currentTarget;
    const idx = fanState.order.findIndex(x => x.id === c.dataset.id);
    const m = new DOMMatrixReadOnly(getComputedStyle(c).transform);
    gesture = { c, id: c.dataset.id, from: idx, slot: idx, pointer: e.pointerId, sx: e.clientX, sy: e.clientY,
      ox: e.clientX - m.m41, oy: e.clientY - m.m42, lastX: e.clientX, rot: 0, moved: false, out: false };
    c.setPointerCapture(e.pointerId);
    c.addEventListener('pointermove', move);
    c.addEventListener('pointerup', release);
    c.addEventListener('pointercancel', release);
  }
  function move(e) {
    const g = gesture;
    if (!g || e.pointerId !== g.pointer) return;
    if (!g.moved) {
      if (Math.hypot(e.clientX - g.sx, e.clientY - g.sy) < 5) return;
      g.moved = true;
      g.c.classList.add('lifted');
      g.c.style.transition = 'box-shadow .25s, --motion-opacity .3s';
      fanCovers.classList.add('sorting');
    }
    const S = fanState.S;
    g.rot = g.rot * 0.82 + clamp((e.clientX - g.lastX) * 1.1, -14, 14) * 0.18;
    g.lastX = e.clientX;
    const x = e.clientX - g.ox, y = e.clientY - g.oy;
    setT(g.c, x, y, g.rot, 1.07);
    const { slots, step, x0 } = fanLayout(fanState.order.length, S);
    const restY = slots[g.from]?.y ?? 0;
    const out = y - restY > S * 0.75;
    if (out !== g.out) {
      g.out = out;
      g.c.classList.toggle('taking-out', out);
      el('fanHint').textContent = out ? 'Release to take it out of the crate' : 'Drag to reorder · drag down to take out';
    }
    const rest = fanState.order.filter(x => x.id !== g.id);
    if (out) {
      layoutRest(rest);
      return;
    }
    g.slot = step ? clamp(Math.round((x - x0) / step), 0, fanState.order.length - 1) : 0;
    const preview = rest.slice();
    preview.splice(g.slot, 0, fanState.order[g.from]);
    layoutFan(g.c, preview);
    g.c.style.zIndex = '999';
  }
  function layoutRest(rest) {
    const { slots } = fanLayout(rest.length, fanState.S);
    rest.forEach((entry, i) => setT(fanState.els.get(entry.id), slots[i].x, slots[i].y, slots[i].r));
  }
  async function release(e) {
    const g = gesture;
    if (!g || e.pointerId !== g.pointer) return;
    gesture = null;
    g.c.removeEventListener('pointermove', move);
    g.c.removeEventListener('pointerup', release);
    g.c.removeEventListener('pointercancel', release);
    g.c.classList.remove('lifted', 'taking-out');
    fanCovers.classList.remove('sorting');
    const st = fanState;
    if (!g.moved) {
      if (e.type === 'pointerup') {
        const from = st.order.findIndex(x => x.id === g.id);
        const albums = st.order.slice(from).map(x => lookup(x.id)).filter(Boolean);
        if (albums.length) play(albums);
        else toast('This album is not in the library right now.');
      }
      return;
    }
    if (g.out && e.type === 'pointerup') {
      const m = new DOMMatrixReadOnly(getComputedStyle(g.c).transform);
      g.c.style.transition = '--motion-transform .6s cubic-bezier(.4,0,.8,.4), --motion-opacity .5s ease';
      setT(g.c, m.m41 + g.rot * 6, window.innerHeight + 40, g.rot * 3 + 18, 0.9);
      g.c.style.setProperty('--motion-opacity', '0');
      st.order = st.order.filter(x => x.id !== g.id);
      st.els.delete(g.id);
      setTimeout(() => g.c.remove(), 650);
      restoreSprings();
      layoutFan();
      await saveFan(update({ type: 'remove', id: st.id, album: g.id }));
      return;
    }
    const entry = st.order[g.from];
    const order = st.order.filter(x => x.id !== g.id);
    const slot = g.out ? g.from : g.slot;
    order.splice(slot, 0, entry);
    st.order = order;
    restoreSprings();
    layoutFan();
    if (slot !== g.from) await saveFan(update({ type: 'reorder', id: st.id, album: g.id, index: slot }));
    else el('fanHint').textContent = st.order.length > 1 ? 'Drag to reorder · drag down to take out' : 'Drag down to take out';
  }
  function restoreSprings() {
    for (const c of fanState.els.values()) c.style.transition = `--motion-transform .6s ${SPRING}, --motion-opacity .4s ease, box-shadow .3s`;
  }
  async function saveFan(saving) {
    try { await saving; } catch { return; }
    const list = fanState && listById(fanState.id);
    if (!list) return;
    el('fanMeta').textContent = list.albums.length ? label(list) : 'Empty — pull covers down in the library to fill it.';
    el('fanPlay').disabled = !list.albums.some(e => lookup(e.id));
    el('fanHint').textContent = list.albums.length > 1 ? 'Drag to reorder · drag down to take out' : list.albums.length ? 'Drag down to take out' : '';
  }

  /* ---------------- fan tools ---------------- */

  el('fanClose').addEventListener('click', collapse);
  fan.querySelector('.fan-scrim').addEventListener('click', collapse);
  el('fanPlay').addEventListener('click', () => {
    const albums = fanState?.order.map(x => lookup(x.id)).filter(Boolean);
    if (albums?.length) play(albums);
  });

  function startRename() {
    if (!fanState) return;
    const input = el('fanNameInput');
    input.value = listById(fanState.id)?.name || '';
    el('fanName').classList.add('hidden');
    input.classList.remove('hidden');
    input.focus();
    input.select();
  }
  let renaming = false;
  async function finishRename(save) {
    const input = el('fanNameInput');
    if (input.classList.contains('hidden') || renaming) return;
    renaming = true;
    const name = input.value.trim();
    if (save && fanState && name && name !== listById(fanState.id)?.name) {
      try {
        await update({ type: 'rename', id: fanState.id, name });
        el('fanName').textContent = name;
      } catch { /* toast already shown */ }
    }
    input.classList.add('hidden');
    el('fanName').classList.remove('hidden');
    renaming = false;
  }
  el('fanRename').addEventListener('click', startRename);
  el('fanName').addEventListener('dblclick', startRename);
  el('fanNameInput').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); finishRename(true); el('fanRename').focus(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); finishRename(false); el('fanRename').focus(); }
  });
  el('fanNameInput').addEventListener('blur', () => finishRename(true));

  let deleteTimer;
  function resetDelete() {
    clearTimeout(deleteTimer);
    el('fanDelete').classList.remove('confirm');
    el('fanDelete').title = 'Delete crate';
  }
  el('fanDelete').addEventListener('click', async () => {
    if (!fanState || fanState.closing) return;
    const btn = el('fanDelete');
    if (!btn.classList.contains('confirm')) {
      btn.classList.add('confirm');
      btn.title = 'Click again to delete';
      el('fanHint').textContent = 'Click again to delete this crate';
      deleteTimer = setTimeout(() => { resetDelete(); saveFan(Promise.resolve()); }, 3200);
      return;
    }
    resetDelete();
    const st = fanState;
    st.closing = true;
    try { await update({ type: 'delete', id: st.id }); } catch { st.closing = false; return; }
    // The records tumble out of sight, the crate goes with them.
    st.order.forEach((entry, i) => {
      const c = st.els.get(entry.id);
      const m = new DOMMatrixReadOnly(getComputedStyle(c).transform);
      const delay = Math.min(i, 12) * 30;
      c.style.transition = `--motion-transform .7s cubic-bezier(.45,0,.9,.5) ${delay}ms, --motion-opacity .6s ease ${delay}ms`;
      setT(c, m.m41 + (i % 2 ? 30 : -30), window.innerHeight + 60, (i % 2 ? 1 : -1) * (14 + i * 2), 0.92);
      c.style.setProperty('--motion-opacity', '0');
    });
    fan.classList.add('closing');
    page.classList.remove('fanned');
    await finishFanMotion(st);
    if (fanState !== st) return;
    closeFan();
    renderShelf();
  });

  window.addEventListener('resize', () => {
    if (isOpen() && !fanState) renderShelf();
    if (fanState && !fanState.closing && !gesture) layoutFan();
  });

  return {
    load: () => update(null).catch(() => {}),
    begin, drag, end, bindFilingSleeve, open, close, isOpen, escape,
  };
}
