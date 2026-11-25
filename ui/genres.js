function createGenres({ invoke, listen, toast, lookup, records, coverUrl, blankColor, onState, back }) {
  const el = id => document.getElementById(id);
  const page = el('genresPage'), board = el('genreBoard');
  const UNSORTED = 'Unsorted';
  let info = { genres: [], albums: {}, suggestions: 0 };
  let inventory = { tags: [], untagged: 0, records: 0, broad: [] };
  let draft = null, dirty = false, uid = 0, tab = 'tags';
  const tagsByKey = () => new Map(inventory.tags.map(t => [t.key, t]));

  function setState(s) {
    info = s;
    onState(s);
    el('reviewCount').textContent = s.suggestions ? s.suggestions : '';
  }
  function status(text) { el('genreStatus').textContent = text || ''; }
  function busy(button, on) { if (button) button.disabled = on; }

  /* ---------------- tabs ---------------- */

  function showTab(name) {
    tab = name;
    for (const b of page.querySelectorAll('#genreTabs [data-tab]')) b.setAttribute('aria-selected', String(b.dataset.tab === name));
    for (const p of page.querySelectorAll('.genre-pane')) p.classList.toggle('hidden', p.dataset.pane !== name);
    if (name === 'sort') renderSort();
    if (name === 'review') loadReview();
  }
  page.querySelectorAll('#genreTabs [data-tab]').forEach(b => b.addEventListener('click', () => showTab(b.dataset.tab)));

  /* ---------------- step 0 + 1: tags and genre cards ---------------- */

  function draftFrom(genres, map, broad) {
    const cards = genres.map(g => ({ uid: ++uid, name: g.name, original: info.genres.some(x => x.name === g.name) ? g.name : null, parentName: g.parent, tags: new Set() }));
    const byName = new Map(cards.map(c => [c.name.toLowerCase(), c]));
    for (const c of cards) {
      c.parent = c.parentName ? byName.get(c.parentName.toLowerCase())?.uid ?? null : null;
      delete c.parentName;
    }
    const unassigned = new Set();
    for (const t of inventory.tags) {
      const card = map[t.key] && byName.get(map[t.key].toLowerCase());
      if (card) card.tags.add(t.key); else unassigned.add(t.key);
    }
    return { cards, unassigned, broad: new Set(broad), renames: new Map() };
  }
  function savedDraft() {
    return draftFrom(info.genres, Object.fromEntries(inventory.tags.filter(t => t.genre).map(t => [t.key, t.genre])), inventory.broad);
  }
  async function loadTags() {
    status('');
    inventory = await invoke('genre_inventory');
    if (!dirty) draft = savedDraft();
    renderBoard();
  }
  function markDirty(on = true) {
    dirty = on;
    el('saveGenres').classList.toggle('hidden', !on);
    el('discardGenres').classList.toggle('hidden', !on);
  }
  const card = id => draft.cards.find(c => c.uid === id);
  const albumCount = keys => { const t = tagsByKey(); let n = 0; for (const k of keys) n += t.get(k)?.albums || 0; return n; };
  function ordered() {
    const tops = draft.cards.filter(c => c.parent == null);
    return tops.flatMap(t => [t, ...draft.cards.filter(c => c.parent === t.uid)]);
  }
  const tagAlbums = key => tagsByKey().get(key)?.albums || 0;
  const tagName = key => tagsByKey().get(key)?.variants[0] || key;
  let sortMode = localStorage.getItem('muso.genreSort') === 'az' ? 'az' : 'count';
  const nameOrder = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
  const parentName = c => c.parent != null ? card(c.parent)?.name : null;

  // One flow of chips: Muso genres (stacks of file tags) and file tags that are not in a genre yet.
  function items() {
    const list = [
      ...draft.cards.map(c => ({ id: 'g' + c.uid, genre: c, name: c.name, albums: albumCount(c.tags) })),
      ...[...draft.unassigned].map(k => ({ id: 't' + k, key: k, name: tagName(k), albums: tagAlbums(k) })),
    ];
    if (sortMode === 'az') list.sort((a, b) => nameOrder.compare(a.genre ? (parentName(a.genre) || a.name) : a.name, b.genre ? (parentName(b.genre) || b.name) : b.name)
      || (a.genre?.parent != null) - (b.genre?.parent != null) || nameOrder.compare(a.name, b.name));
    else list.sort((a, b) => !!b.genre - !!a.genre || b.albums - a.albums || nameOrder.compare(a.name, b.name));
    return list;
  }
  function chipEl(it) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'tchip';
    b.dataset.id = it.id;
    const name = document.createElement('span');
    const n = document.createElement('small');
    n.textContent = it.albums;
    if (it.genre) {
      const c = it.genre;
      b.classList.add('genre');
      b.dataset.uid = c.uid;
      b.style.setProperty('--depth', Math.min(c.tags.size, 3));
      const parent = parentName(c);
      if (parent) { const p = document.createElement('i'); p.textContent = parent + ' ›'; name.append(p); }
      name.append(c.name);
      const members = [...c.tags].map(tagName);
      b.ariaDescription = (members.length ? members.join(' · ') : 'No file tags yet – albums arrive here in step 2')
        + '\nClick to rename, nest or take tags out';
      b.append(name, n);
      if (c.tags.size > 1) { const s = document.createElement('em'); s.textContent = c.tags.size; s.ariaDescription = c.tags.size + ' file tags'; b.append(s); }
      if ([...c.tags].some(k => draft.broad.has(k))) b.classList.add('broad');
    } else {
      const t = tagsByKey().get(it.key);
      b.dataset.key = it.key;
      name.textContent = it.name;
      b.ariaDescription = (t?.variants.join(' · ') || it.key) + (t?.artists.length ? '\n' + t.artists.join(', ') : '') + '\nDrag onto another tag to merge them into one genre';
      b.append(name, n);
      if (draft.broad.has(it.key)) b.classList.add('broad');
    }
    return b;
  }
  function renderBoard() {
    const tags = inventory.tags.length;
    const files = inventory.records - inventory.untagged;
    el('tagSummary').textContent = tags
      ? `Your files use ${tags} different genre tags on ${files} albums` + (inventory.untagged ? ` · ${inventory.untagged} albums have no genre tag` : '')
        + (draft.cards.length ? ` · ${draft.cards.length} Muso genres · ${draft.unassigned.size} tags left` : '')
      : 'None of your albums has a genre tag in its files.';
    el('genreSort').textContent = sortMode === 'az' ? 'A–Z' : 'Most albums';
    const flow = document.createElement('div');
    flow.className = 'tag-flow';
    flow.append(...items().map((it, i) => { const c = chipEl(it); c.style.setProperty('--n', i % 24); return c; }));
    board.replaceChildren(flow);
    if (!tags && !draft.cards.length) {
      const p = document.createElement('p');
      p.className = 'muted genre-empty';
      p.textContent = 'Scan a folder with tagged music first.';
      board.append(p);
    }
  }
  el('genreSort').addEventListener('click', () => {
    sortMode = sortMode === 'az' ? 'count' : 'az';
    localStorage.setItem('muso.genreSort', sortMode);
    animate(() => {});
  });

  // FLIP: chips glide to their new place; tags that were absorbed fly into their genre.
  function rects() {
    const m = new Map();
    for (const c of board.querySelectorAll('.tchip')) m.set(c.dataset.id, c.getBoundingClientRect());
    return m;
  }
  function animate(change, opts = {}) {
    const before = rects();
    closePop();
    change();
    renderBoard();
    const born = opts.born;
    const quick = matchMedia('(prefers-reduced-motion: reduce)').matches;
    for (const c of board.querySelectorAll('.tchip')) {
      const was = before.get(c.dataset.id) || (born && c.dataset.uid == born.uid && before.get('t' + born.from));
      const now = c.getBoundingClientRect();
      if (!was) { if (!quick) c.animate([{ transform: 'scale(.6)', opacity: 0 }, { transform: 'scale(1.06)', opacity: 1, offset: .7 }, { transform: 'none' }], { duration: 420, easing: 'cubic-bezier(.3,1.35,.5,1)' }); continue; }
      const dx = was.left - now.left, dy = was.top - now.top;
      if (!quick && (dx || dy)) c.animate([{ transform: `translate(${dx}px,${dy}px)` }, { transform: 'none' }], { duration: 480, easing: 'cubic-bezier(.3,1.2,.5,1)' });
    }
    if (quick) return;
    for (const [id, r] of before) {
      if (!id.startsWith('t') || board.querySelector(`.tchip[data-id="${CSS.escape(id)}"]`)) continue;
      const owner = draft.cards.find(c => c.tags.has(id.slice(1)));
      const target = owner && board.querySelector(`.tchip[data-uid="${owner.uid}"]`);
      if (!target) continue;
      const t = target.getBoundingClientRect();
      const g = document.createElement('div');
      g.className = 'tchip flying';
      g.textContent = tagName(id.slice(1));
      Object.assign(g.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
      document.body.append(g);
      const tx = t.left + t.width / 2 - (r.left + r.width / 2), ty = t.top + t.height / 2 - (r.top + r.height / 2);
      g.animate([
        { transform: 'none', opacity: 1 },
        { transform: `translate(${tx * .55}px,${ty * .55 - 26}px) rotate(${tx > 0 ? 6 : -6}deg) scale(1.05)`, opacity: 1, offset: .55 },
        { transform: `translate(${tx}px,${ty}px) scale(.45)`, opacity: 0 },
      ], { duration: 620, easing: 'cubic-bezier(.45,.05,.35,1)' }).onfinish = () => g.remove();
      target.animate([{ transform: 'none' }, { transform: 'scale(1.12)', offset: .8 }, { transform: 'none' }], { duration: 700, easing: 'ease-out' });
    }
  }

  function genreFromTag(key) {
    const name = tagName(key);
    const same = draft.cards.find(c => c.name.toLowerCase() === name.toLowerCase());
    if (same) { draft.unassigned.delete(key); same.tags.add(key); return same; }
    const c = { uid: ++uid, name, original: null, parent: null, tags: new Set([key]) };
    draft.unassigned.delete(key);
    draft.cards.push(c);
    return c;
  }
  function takeTag(key) {
    draft.unassigned.delete(key);
    for (const c of draft.cards) c.tags.delete(key);
  }
  function merge(from, into) {
    for (const k of from.tags) into.tags.add(k);
    for (const x of draft.cards) if (x.parent === from.uid) x.parent = into.parent == null ? into.uid : null;
    if (from.original) draft.renames.set(from.original, into);
    for (const [old, target] of draft.renames) if (target === from) draft.renames.set(old, into);
    draft.cards = draft.cards.filter(x => x !== from);
  }
  function combine(src, dst) {
    let born;
    animate(() => {
      const target = dst.uid ? card(Number(dst.uid)) : null;
      if (src.uid && target) merge(card(Number(src.uid)), target);
      else if (src.uid) { const g = card(Number(src.uid)); takeTag(dst.key); g.tags.add(dst.key); }
      else if (target) { takeTag(src.key); target.tags.add(src.key); }
      else { takeTag(src.key); const g = genreFromTag(dst.key); g.tags.add(src.key); born = { uid: g.uid, from: dst.key }; }
    }, { get born() { return born; } });
    markDirty();
  }

  // Chips are picked up and carried.
  let carry = null;
  board.addEventListener('pointerdown', e => {
    const chip = e.target.closest('.tchip');
    if (e.button !== 0 || !chip) return;
    carry = { src: { key: chip.dataset.key, uid: chip.dataset.uid }, el: chip, x: e.clientX, y: e.clientY, pointer: e.pointerId, moved: false, over: null };
    chip.setPointerCapture(e.pointerId);
  });
  board.addEventListener('pointermove', e => {
    if (!carry || e.pointerId !== carry.pointer) return;
    if (!carry.moved) {
      if (Math.hypot(e.clientX - carry.x, e.clientY - carry.y) < 5) return;
      carry.moved = true;
      closePop();
      const g = carry.el.cloneNode(true);
      g.className = carry.el.className + ' genre-ghost';
      g.removeAttribute('data-id');
      document.body.append(g);
      carry.ghost = g;
      carry.el.classList.add('carried');
      board.classList.add('carrying');
    }
    carry.tilt = (carry.tilt || 0) * .8 + Math.max(-14, Math.min(14, (e.clientX - (carry.lx ?? e.clientX)) * .9)) * .2;
    carry.lx = e.clientX;
    carry.ghost.style.transform = `translate(${e.clientX}px,${e.clientY}px) translate(-50%,-60%) rotate(${carry.tilt.toFixed(1)}deg)`;
    const over = document.elementFromPoint(e.clientX, e.clientY)?.closest('.tchip:not(.carried):not(.genre-ghost)');
    const valid = over && board.contains(over) ? over : null;
    if (valid !== carry.over) { carry.over?.classList.remove('over'); valid?.classList.add('over'); carry.over = valid; }
    const r = board.getBoundingClientRect();
    if (e.clientY > r.bottom - 50) board.scrollBy(0, 14);
    else if (e.clientY < r.top + 50) board.scrollBy(0, -14);
  });
  function drop(e) {
    if (!carry || e.pointerId !== carry.pointer) return;
    const c = carry;
    carry = null;
    if (!c.moved) {
      if (e.type === 'pointerup' && c.src.uid) openPop(Number(c.src.uid));
      return;
    }
    c.ghost.remove();
    c.el.classList.remove('carried');
    board.classList.remove('carrying');
    c.over?.classList.remove('over');
    if (!c.over || e.type !== 'pointerup') return;
    combine(c.src, { key: c.over.dataset.key, uid: c.over.dataset.uid });
  }
  board.addEventListener('pointerup', drop);
  board.addEventListener('pointercancel', drop);
  board.addEventListener('lostpointercapture', drop);

  /* ---------------- genre popover ---------------- */

  const pop = el('genrePop');
  let popUid = null;
  function openPop(id) {
    if (popUid != null && popUid !== id) commitName();
    const c = card(id);
    if (!c) return;
    popUid = id;
    el('popName').value = c.name;
    const level = el('popLevel');
    level.replaceChildren(new Option('Top level', ''));
    const hasChildren = draft.cards.some(x => x.parent === c.uid);
    for (const t of draft.cards.filter(x => x.parent == null && x.uid !== c.uid).sort((a, b) => nameOrder.compare(a.name, b.name))) {
      level.append(new Option('Under ' + t.name, t.uid, false, c.parent === t.uid));
    }
    level.disabled = hasChildren;
    level.ariaDescription = hasChildren ? 'Has its own subgenres' : '';
    el('popTags').replaceChildren(...[...c.tags].sort((a, b) => tagAlbums(b) - tagAlbums(a)).map(k => {
      const li = document.createElement('li');
      const s = document.createElement('span');
      s.textContent = tagName(k);
      const n = document.createElement('small');
      n.textContent = tagAlbums(k);
      const x = document.createElement('button');
      x.type = 'button';
      x.textContent = '×';
      x.ariaDescription = 'Take this tag out of the genre';
      x.setAttribute('aria-label', 'Take ' + tagName(k) + ' out');
      x.addEventListener('click', () => { animate(() => { takeTag(k); draft.unassigned.add(k); }); markDirty(); openPop(id); });
      li.append(s, n, x);
      return li;
    }));
    if (!c.tags.size) {
      const li = document.createElement('li');
      li.className = 'muted';
      li.textContent = 'No file tags – the AI can sort albums into it in step 2.';
      el('popTags').append(li);
    }
    const chip = board.querySelector(`.tchip[data-uid="${id}"]`);
    pop.classList.remove('hidden');
    if (chip) {
      const r = chip.getBoundingClientRect(), page = el('genresPage').getBoundingClientRect();
      const left = Math.min(Math.max(12, r.left - page.left), page.width - pop.offsetWidth - 12);
      const below = r.bottom - page.top + 8;
      pop.style.left = left + 'px';
      pop.style.top = (below + pop.offsetHeight > page.height - 12 ? r.top - page.top - pop.offsetHeight - 8 : below) + 'px';
      for (const x of board.querySelectorAll('.tchip.open')) x.classList.remove('open');
      chip.classList.add('open');
    }
  }
  function closePop() {
    if (pop.classList.contains('hidden')) return false;
    commitName();
    pop.classList.add('hidden');
    for (const x of board.querySelectorAll('.tchip.open')) x.classList.remove('open');
    popUid = null;
    return true;
  }
  function commitName() {
    const c = popUid != null && card(popUid);
    const name = el('popName').value.trim();
    if (!c || !name || name === c.name) return;
    c.name = name;
    markDirty();
    const chip = board.querySelector(`.tchip[data-uid="${c.uid}"]`);
    if (chip) chip.replaceWith(chipEl({ id: 'g' + c.uid, genre: c, name: c.name, albums: albumCount(c.tags) }));
  }
  el('popName').addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); closePop(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); el('popName').value = card(popUid)?.name || ''; closePop(); }
  });
  el('popLevel').addEventListener('change', () => {
    const c = card(popUid);
    if (!c) return;
    const id = popUid;
    commitName();
    animate(() => { c.parent = el('popLevel').value ? Number(el('popLevel').value) : null; });
    markDirty();
    openPop(id);
  });
  el('popDissolve').addEventListener('click', () => {
    const c = card(popUid);
    if (!c) return;
    animate(() => {
      for (const k of c.tags) draft.unassigned.add(k);
      for (const x of draft.cards) if (x.parent === c.uid) x.parent = null;
      draft.cards = draft.cards.filter(x => x !== c);
    });
    markDirty();
  });
  document.addEventListener('pointerdown', e => {
    if (!pop.classList.contains('hidden') && !pop.contains(e.target) && !e.target.closest?.('.tchip.genre')) closePop();
  }, true);
  board.addEventListener('scroll', () => closePop());

  el('addGenre').addEventListener('click', () => {
    let name = 'New genre', n = 2;
    while (draft.cards.some(c => c.name.toLowerCase() === name.toLowerCase())) name = 'New genre ' + n++;
    const c = { uid: ++uid, name, original: null, parent: null, tags: new Set() };
    animate(() => draft.cards.push(c));
    markDirty();
    board.querySelector(`.tchip[data-uid="${c.uid}"]`)?.scrollIntoView({ block: 'nearest' });
    openPop(c.uid);
    el('popName').select();
  });
  el('discardGenres').addEventListener('click', () => { animate(() => { draft = savedDraft(); }); markDirty(false); status(''); });
  el('saveGenres').addEventListener('click', async () => {
    commitName();
    const names = new Set();
    for (const c of draft.cards) {
      const n = c.name.trim().toLowerCase();
      if (!n) return toast('Every genre needs a name.');
      if (names.has(n)) return toast(`“${c.name}” appears twice.`);
      names.add(n);
    }
    const genres = ordered().map(c => ({ name: c.name.trim(), parent: c.parent == null ? null : card(c.parent).name.trim() }));
    const map = {};
    for (const c of draft.cards) for (const k of c.tags) map[k] = c.name.trim();
    const renames = {};
    for (const c of draft.cards) if (c.original && c.original !== c.name.trim()) renames[c.original] = c.name.trim();
    for (const [old, target] of draft.renames) if (draft.cards.includes(target)) renames[old] = target.name.trim();
    busy(el('saveGenres'), true);
    try {
      setState(await invoke('genre_save', { genres, map, renames, broad: [...draft.broad] }));
      markDirty(false);
      await loadTags();
      status('Genres saved. The genre tags in your music files are unchanged.');
    } catch (e) { toast(String(e), 5000); }
    finally { busy(el('saveGenres'), false); }
  });

  // The AI's suggestion is played back as if someone sorted the tags by hand.
  let playing = 0;
  async function playProposal(target) {
    const run = ++playing;
    board.classList.add('ai-sorting');
    animate(() => {
      draft = { cards: [], unassigned: new Set(inventory.tags.map(t => t.key)), broad: target.broad, renames: new Map() };
    });
    await new Promise(r => setTimeout(r, 350));
    const order = target.cards.slice().sort((a, b) => albumCount(b.tags) - albumCount(a.tags));
    for (const c of order) {
      if (run !== playing) return;
      const tags = [...c.tags].sort((a, b) => tagAlbums(b) - tagAlbums(a));
      const host = tags[0];
      animate(() => {
        for (const k of c.tags) draft.unassigned.delete(k);
        draft.cards.push({ ...c, parent: null });
      }, { born: host ? { uid: c.uid, from: host } : undefined });
      await new Promise(r => setTimeout(r, Math.max(140, 420 - order.length * 8)));
    }
    if (run !== playing) return;
    animate(() => { draft = target; });
    board.classList.remove('ai-sorting');
  }
  el('proposeGenres').addEventListener('click', async () => {
    if (dirty && !confirmReplace()) return;
    const b = el('proposeGenres');
    busy(b, true);
    board.classList.add('thinking');
    status('The AI is reading your genre tags…');
    try {
      const p = await invoke('genre_propose');
      const target = draftFrom(p.genres, p.map, p.broad);
      for (const c of target.cards) c.original = info.genres.find(g => g.name.toLowerCase() === c.name.toLowerCase())?.name ?? null;
      board.classList.remove('thinking');
      status(`Sorting ${inventory.tags.length} tags into ${p.genres.length} genres…`);
      await playProposal(target);
      markDirty();
      status(`Suggestion: ${p.genres.length} genres. Drag chips onto each other, click a genre to rename or take tags out – nothing is saved until you press Save.`);
    } catch (e) { status(''); toast(String(e), 6000); }
    finally { busy(b, false); board.classList.remove('thinking'); }
  });
  let replaceAsked = 0;
  function confirmReplace() {
    if (Date.now() - replaceAsked < 4000) return true;
    replaceAsked = Date.now();
    toast('Unsaved changes – click again to replace them with a new suggestion.', 4000);
    return false;
  }

  /* ---------------- step 2: sort albums ---------------- */

  const selected = new Set();
  const instructions = el('genreInstructions');
  const instructionKey = 'muso.genreInstructions';
  instructions.value = localStorage.getItem(instructionKey) ?? instructions.defaultValue;
  instructions.addEventListener('input', () => localStorage.setItem(instructionKey, instructions.value));
  let sorting = false;
  function buckets() {
    const m = new Map();
    for (const a of records()) {
      const g = info.albums[a.id];
      if (g?.origin === 'user') continue;
      const name = g?.genre || UNSORTED;
      if (!m.has(name)) m.set(name, []);
      m.get(name).push(a.id);
    }
    return m;
  }
  function presetBuckets(names) {
    selected.clear();
    if (names) { names.forEach(n => selected.add(n)); return; }
    const t = tagsByKey();
    for (const key of inventory.broad) { const g = t.get(key)?.genre; if (g) selected.add(g); }
    selected.add(UNSORTED);
  }
  function renderSort() {
    const ready = info.genres.length > 0;
    el('sortSetup').classList.toggle('hidden', !ready);
    el('sortMissing').classList.toggle('hidden', ready);
    if (!ready) return;
    const m = buckets();
    const order = new Map(info.genres.map((g, i) => [g.name, i]));
    const names = [...m.keys()].sort((a, b) => (a === UNSORTED) - (b === UNSORTED) || (order.get(a) ?? 1e6) - (order.get(b) ?? 1e6) || a.localeCompare(b));
    el('sortBuckets').replaceChildren(...names.map(name => {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'bucket';
      b.setAttribute('aria-pressed', String(selected.has(name)));
      const label = document.createElement('span');
      label.textContent = name;
      const n = document.createElement('small');
      n.textContent = m.get(name).length;
      b.append(label, n);
      b.disabled = sorting;
      b.addEventListener('click', () => { selected.has(name) ? selected.delete(name) : selected.add(name); renderSort(); });
      return b;
    }));
    const ids = chosenIds(m);
    const requests = Math.ceil(ids.length / 25);
    el('sortEstimate').textContent = ids.length
      ? `${ids.length} albums · ${requests} AI request${requests === 1 ? '' : 's'}` + (el('sortResearch').checked ? ' · one additional web search per uncertain album. Above 50, Muso asks first.' : '')
      : 'Choose at least one genre.';
    el('startSort').disabled = sorting || !ids.length;
    instructions.readOnly = sorting;
    el('sortResearch').disabled = sorting;
  }
  const chosenIds = (m = buckets()) => [...selected].flatMap(n => m.get(n) || []);
  el('sortResearch').addEventListener('change', renderSort);
  el('startSort').addEventListener('click', async () => {
    const ids = chosenIds();
    if (!ids.length || sorting) return;
    sorting = true;
    status('');
    el('cancelSort').classList.remove('hidden');
    el('sortProgress').classList.remove('hidden');
    progress({ done: 0, total: ids.length, stage: 'Sorting', uncertain: 0 });
    renderSort();
    try {
      const result = await invoke('genre_classify', {
        albumIds: ids, research: el('sortResearch').checked, instructions: instructions.value.trim(),
      });
      setState(await invoke('genre_state'));
      if (el('sortResearch').checked) status(`${result.uncertain} uncertain · web lookups attempted: ${result.attempted} of ${result.uncertain}`);
      toast(result.suggestions ? `${result.suggestions} suggestions ready to review` : 'No new genre suggestions.');
      if (result.suggestions) showTab('review');
    } catch (e) { toast(String(e), 6000); }
    finally {
      sorting = false;
      el('cancelSort').classList.add('hidden');
      el('sortProgress').classList.add('hidden');
      if (tab === 'sort') renderSort();
      setState(await invoke('genre_state').catch(() => info));
    }
  });
  el('cancelSort').addEventListener('click', () => { invoke('genre_cancel'); el('sortProgressText').textContent = 'Stopping after this step…'; });
  function progress(p) {
    const share = p.total ? p.done / p.total : 0;
    el('sortProgress').querySelector('i').style.transform = `scaleX(${share})`;
    el('sortProgressText').textContent = p.stage === 'Researching'
      ? `Looking up uncertain album ${p.done + 1} of ${p.total}`
      : `${p.stage} · ${p.done} / ${p.total}` + (el('sortResearch').checked ? ` · ${p.uncertain} uncertain so far` : '');
  }
  listen('genre-progress', ({ payload }) => { if (sorting) progress(payload); });

  /* ---------------- step 3: review ---------------- */

  let suggestions = [];
  async function loadReview() {
    suggestions = await invoke('genre_suggestions');
    renderReview();
  }
  function genreOptions(select, chosen, extra) {
    const kids = p => info.genres.filter(g => g.parent === p);
    for (const top of info.genres.filter(g => !g.parent)) {
      select.append(new Option(top.name, top.name, false, top.name === chosen));
      for (const k of kids(top.name)) select.append(new Option('\u2003' + k.name, k.name, false, k.name === chosen));
    }
    if (extra && !info.genres.some(g => g.name.toLowerCase() === extra.toLowerCase())) {
      select.append(new Option(extra + ' (new)', extra, false, extra === chosen));
    }
  }
  function renderReview() {
    const sure = suggestions.filter(s => s.confidence >= .8).length;
    el('reviewSummary').textContent = suggestions.length
      ? `${suggestions.length} suggestions · ${sure} at 80 % or more. Nothing changes until you accept.`
      : 'No open suggestions. Sort albums in step 2.';
    el('acceptSure').disabled = !sure;
    el('acceptAll').disabled = el('rejectAll').disabled = !suggestions.length;
    el('reviewList').replaceChildren(...suggestions.map(row));
  }
  function row(s) {
    const li = document.createElement('li');
    li.className = 'rrow';
    li.dataset.id = s.album_id;
    const a = lookup(s.album_id);
    const thumb = document.createElement('span');
    thumb.className = 'rthumb';
    thumb.style.setProperty('--blank', blankColor(s.album_id));
    if (a?.art) { const img = new Image(); img.src = coverUrl(a, 'thumb'); img.alt = ''; img.loading = 'lazy'; thumb.append(img); }
    const text = document.createElement('div');
    text.className = 'rtext';
    const title = document.createElement('div');
    title.className = 'rtitle';
    title.textContent = s.artist + ' — ' + s.title;
    const reason = document.createElement('div');
    reason.className = 'rreason';
    reason.textContent = s.reason + (s.researched ? ' · checked on the web' : '');
    text.append(title, reason);
    const from = document.createElement('span');
    from.className = 'rfrom';
    from.textContent = s.current || s.tag || '—';
    from.ariaDescription = s.tag ? 'Tag: ' + s.tag : 'No tag';
    const to = document.createElement('select');
    to.className = 'rto';
    to.setAttribute('aria-label', 'Genre for ' + s.title);
    genreOptions(to, s.genre, s.new_genre ? s.genre : null);
    if (s.new_genre) li.classList.add('new');
    const conf = document.createElement('span');
    conf.className = 'rconf';
    conf.textContent = Math.round(s.confidence * 100) + ' %';
    conf.style.setProperty('--c', s.confidence);
    const ok = document.createElement('button');
    ok.type = 'button'; ok.className = 'rok'; ok.textContent = '✓'; ok.ariaDescription = 'Accept'; ok.setAttribute('aria-label', 'Accept ' + to.value + ' for ' + s.title);
    ok.addEventListener('click', () => decide([{ album_id: s.album_id, genre: to.value }], []));
    const no = document.createElement('button');
    no.type = 'button'; no.className = 'rno'; no.textContent = '×'; no.ariaDescription = 'Reject'; no.setAttribute('aria-label', 'Reject suggestion for ' + s.title);
    no.addEventListener('click', () => decide([], [s.album_id]));
    li.append(thumb, text, from, document.createTextNode('→'), to, conf, ok, no);
    return li;
  }
  async function decide(accept, reject) {
    const ids = new Set([...accept.map(a => a.album_id), ...reject]);
    for (const li of el('reviewList').children) if (ids.has(li.dataset.id)) li.classList.add(reject.includes(li.dataset.id) ? 'gone-no' : 'gone-ok');
    try {
      setState(await invoke('genre_review', { accept, reject }));
      await new Promise(r => setTimeout(r, 260));
      suggestions = suggestions.filter(s => !ids.has(s.album_id));
      renderReview();
    } catch (e) { toast(String(e), 5000); renderReview(); }
  }
  const chosen = s => el('reviewList').querySelector(`.rrow[data-id="${CSS.escape(s.album_id)}"] .rto`)?.value || s.genre;
  el('acceptSure').addEventListener('click', () => decide(suggestions.filter(s => s.confidence >= .8).map(s => ({ album_id: s.album_id, genre: chosen(s) })), []));
  el('acceptAll').addEventListener('click', () => decide(suggestions.map(s => ({ album_id: s.album_id, genre: chosen(s) })), []));
  el('rejectAll').addEventListener('click', () => decide([], suggestions.map(s => s.album_id)));

  /* ---------------- page ---------------- */

  async function open(opts = {}) {
    page.classList.remove('hidden');
    el('genresBack').focus({ preventScroll: true });
    try {
      setState(await invoke('genre_state'));
      await loadTags();
    } catch (e) { toast(String(e), 5000); }
    presetBuckets(opts.buckets);
    showTab(opts.tab || (info.genres.length ? (info.suggestions ? 'review' : 'sort') : 'tags'));
  }
  function close() { page.classList.add('hidden'); }
  function escape() {
    if (carry) return;
    if (closePop()) return;
    back();
  }
  el('genresBack').addEventListener('click', () => back());
  return { open, close, escape, setState: s => { info = s; } };
}
