'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const ASSET = navigator.userAgent.includes('Windows') ? 'http://muso.localhost/' : 'muso://localhost/';
const asset = (rel) => ASSET + rel.split('/').map(encodeURIComponent).join('/');
const coverUrl = (album, folder) => asset(folder + '/' + (album.cover_file || album.id + '.jpg'));
const $ = (id) => document.getElementById(id);

const WINDOW = 9; // sleeves rendered on each side of the focused one
const ATMOS_INTERVAL = 45000;

const state = {
  albums: [],
  pos: 0,
  target: 0,
  view: 'library',
  viewAlbum: null,
  playing: null, // { album, tracks, index, paused, ended }
  size: 420,
};

/* ---------------- helpers ---------------- */

function hue(id) {
  return parseInt(id.slice(0, 6), 16) % 360;
}
function blankColor(id) {
  return `hsl(${hue(id)}, 14%, 20%)`;
}
let toastTimer;
function toast(msg, ms = 2600) {
  const t = $('toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), ms);
}
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
function show(el, on = true) {
  if (el.classList.contains('animated-overlay')) el.inert = !on;
  el.classList.toggle('hidden', !on);
}

/* ---------------- crate ---------------- */

const crate = $('crate');
const pool = new Map();
let crateMotion = null;
const libraryIndex = $('libraryIndex');
const indexLetters = $('indexLetters');
const letters = '#ABCDEFGHIJKLMNOPQRSTUVWXYZ';
const letterButtons = new Map();
const titleOrder = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
const savedMode = localStorage.getItem('muso.indexMode');
let libraryMode = ['album', 'genre'].includes(savedMode) ? savedMode : 'artist';
let crateMode = 'order';
const UNSORTED = 'Unsorted';
let genreInfo = { genres: [], albums: {}, suggestions: 0 };
const indexGenres = $('indexGenres'), indexSubs = $('indexSubs');
let genreMarks = [], currentGroup, currentSub;
let artistAlbums = [], titleAlbums = [];
let preferredMember;
let currentLetter;
const newButton = $('newAlbums');
const recentWindow = 28 * 24 * 60 * 60 * 1000;
let browsingNew = false, recentAlbums = [], expiryTimer;
let activeCrate = null, cratePosition = null, libraryReturn = null;
const crateKey = 'muso.browsedCrate';
const indexMode = () => activeCrate ? crateMode : libraryMode;
function scopedAlbums(normal) {
  if (!activeCrate) return normal;
  if (crateMode === 'order') return activeCrate.albums.map(entry => albumsById.get(entry.id)).filter(Boolean);
  const ids = new Set(activeCrate.albums.map(a => a.id));
  return [...albumsById.values()].filter(a => ids.has(a.id));
}

function albumInitial(album, byTitle) {
  const name = byTitle ? album.title.trim() : album.artist.trim().replace(/^the /i, '').trimStart();
  const first = name.normalize('NFKD').toUpperCase().charAt(0);
  return /^[A-Z]$/.test(first) ? first : '#';
}

for (const letter of letters) {
  const button = document.createElement('button');
  const glyph = document.createElement('span');
  glyph.textContent = letter;
  button.append(glyph);
  button.tabIndex = -1;
  button.disabled = true;
  button.addEventListener('click', e => {
    if (libraryGestures.active) return;
    if (browsingNew) leaveNew();
    go(Number(button.dataset.index));
    if (e.detail) button.blur();
  });
  indexLetters.append(button);
  letterButtons.set(letter, button);
}
indexLetters.addEventListener('keydown', e => {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return;
  e.preventDefault();
  e.stopPropagation();
  const enabled = [...indexLetters.querySelectorAll('button:enabled')];
  let i = enabled.indexOf(e.target);
  if (e.key === 'Home') i = 0;
  else if (e.key === 'End') i = enabled.length - 1;
  else i = clamp(i + (e.key === 'ArrowLeft' ? -1 : 1), 0, enabled.length - 1);
  enabled[i].focus();
  enabled[i].click();
});

const modeLabels = { order: 'Order', artist: 'Artist', album: 'Album', genre: 'Genre' };
function nextIndexMode() {
  const modes = activeCrate ? ['order', 'artist', 'album', 'genre'] : ['artist', 'album', 'genre'];
  return modes[(modes.indexOf(indexMode()) + 1) % modes.length];
}
const modeAlbums = () => indexMode() === 'artist' ? artistAlbums : titleAlbums;

const genreKey = s => s.toLowerCase().replace(/&/g, ' and ').split(/[\s\-_.]+/).filter(Boolean).join(' ');
function annotateGenres(albums) {
  const parents = new Map(genreInfo.genres.map(g => [g.name.toLowerCase(), g.parent]));
  // Before a genre list exists, spelling variants of the same raw tag share one entry.
  const spelling = new Map();
  for (const a of albums) {
    const g = genreInfo.albums[a.id];
    let name = g?.genre || null;
    if (name && g.origin === 'tag') {
      const key = genreKey(name);
      if (!spelling.has(key)) spelling.set(key, name);
      name = spelling.get(key);
    }
    a.genreName = name;
    a.genreOrigin = g?.origin || 'none';
    a.genreTag = g?.tag ?? a.genre ?? null;
    const parent = a.genreName ? parents.get(a.genreName.toLowerCase()) : null;
    a.genreGroup = parent || a.genreName || UNSORTED;
    a.genreSub = parent ? a.genreName : null;
  }
}

// Genre order follows the saved genre list; raw tags (before tidying) are alphabetical.
function indexByGenre(albums) {
  const rank = new Map(genreInfo.genres.map((g, i) => [g.name.toLowerCase(), i]));
  const place = name => name === UNSORTED ? Infinity : rank.get(name.toLowerCase()) ?? 1e6;
  const groups = new Map();
  for (const a of albums) {
    a.initial = undefined;
    if (!groups.has(a.genreGroup)) groups.set(a.genreGroup, []);
    groups.get(a.genreGroup).push(a);
  }
  const names = [...groups.keys()].sort((x, y) => place(x) - place(y) || titleOrder.compare(x, y));
  const out = [];
  genreMarks = names.map(name => {
    const group = groups.get(name);
    const subPlace = a => a.genreSub ? place(a.genreSub) : -1;
    group.sort((a, b) => subPlace(a) - subPlace(b) || (a.genreSub && b.genreSub ? titleOrder.compare(a.genreSub, b.genreSub) : 0));
    const start = out.length;
    const subs = [];
    group.forEach((a, i) => { if (a.genreSub && subs.at(-1)?.name !== a.genreSub) subs.push({ name: a.genreSub, index: start + i, count: 0 }); if (a.genreSub) subs.at(-1).count++; });
    out.push(...group);
    return { name, index: start, count: group.length, subs };
  });
  indexGenres.replaceChildren(...genreMarks.map(m => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = m.name;
    b.dataset.index = m.index;
    b.dataset.name = m.name;
    b.ariaDescription = m.count + (m.count === 1 ? ' album' : ' albums') + (m.subs.length ? ' · click again for subgenres' : '');
    if (m.name === UNSORTED) b.classList.add('unsorted');
    return b;
  }));
  currentGroup = currentSub = undefined;
  show(indexSubs, false);
  return out;
}
function markGenre(a) {
  if (indexMode() !== 'genre' || browsingNew) return;
  if (a.genreGroup !== currentGroup) {
    currentGroup = a.genreGroup;
    let active;
    for (const b of indexGenres.children) {
      const on = b.dataset.name === currentGroup;
      b.toggleAttribute('aria-current', on);
      if (on) active = b;
    }
    if (active) indexGenres.scrollTo({ left: active.offsetLeft - indexGenres.clientWidth / 2 + active.offsetWidth / 2, behavior: 'smooth' });
    if (!indexSubs.classList.contains('hidden')) renderSubs();
  }
  if (a.genreSub !== currentSub) {
    currentSub = a.genreSub;
    for (const b of indexSubs.children) b.toggleAttribute('aria-current', b.dataset.name === (currentSub ?? ''));
  }
}
function renderSubs() {
  const m = genreMarks.find(x => x.name === currentGroup);
  if (!m?.subs.length) { show(indexSubs, false); return; }
  const all = document.createElement('button');
  all.type = 'button';
  all.textContent = 'All ' + m.name;
  all.dataset.index = m.index;
  all.dataset.name = '';
  indexSubs.replaceChildren(all, ...m.subs.map(s => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = s.name;
    b.ariaDescription = s.count + (s.count === 1 ? ' album' : ' albums');
    b.dataset.index = s.index;
    b.dataset.name = s.name;
    return b;
  }));
  for (const b of indexSubs.children) b.toggleAttribute('aria-current', b.dataset.name === (currentSub ?? ''));
  show(indexSubs);
}
// The genre bar scrolls with the wheel and can be dragged sideways; a drag never counts as a click.
let barDrag = null, barGlide = 0;
function glideBar(v) {
  cancelAnimationFrame(barGlide);
  const step = () => {
    if (Math.abs(v) < .3) return;
    indexGenres.scrollLeft -= v;
    v *= .92;
    barGlide = requestAnimationFrame(step);
  };
  barGlide = requestAnimationFrame(step);
}
indexGenres.addEventListener('wheel', e => {
  e.preventDefault();
  e.stopPropagation();
  cancelAnimationFrame(barGlide);
  const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
  indexGenres.scrollLeft += e.deltaMode === 1 ? d * 30 : d;
}, { passive: false });
indexGenres.addEventListener('pointerdown', e => {
  if (e.button !== 0) return;
  cancelAnimationFrame(barGlide);
  barDrag = { x: e.clientX, left: indexGenres.scrollLeft, last: e.clientX, v: 0, moved: false, id: e.pointerId };
});
indexGenres.addEventListener('pointermove', e => {
  if (!barDrag || e.pointerId !== barDrag.id) return;
  const dx = e.clientX - barDrag.x;
  if (!barDrag.moved && Math.abs(dx) < 5) return;
  if (!barDrag.moved) { barDrag.moved = true; indexGenres.setPointerCapture(e.pointerId); indexGenres.classList.add('dragging'); }
  barDrag.v = e.clientX - barDrag.last;
  barDrag.last = e.clientX;
  indexGenres.scrollLeft = barDrag.left - dx;
});
function endBarDrag(e) {
  if (!barDrag || e.pointerId !== barDrag.id) return;
  const d = barDrag;
  barDrag = null;
  indexGenres.classList.remove('dragging');
  if (!d.moved) return;
  glideBar(d.v);
  barDraggedAt = performance.now();
}
let barDraggedAt = 0;
indexGenres.addEventListener('pointerup', endBarDrag);
indexGenres.addEventListener('pointercancel', endBarDrag);
indexGenres.addEventListener('click', e => {
  if (performance.now() - barDraggedAt < 250) return;
  const b = e.target.closest('button');
  if (!b || libraryGestures.active) return;
  if (browsingNew) leaveNew();
  if (b.dataset.name === currentGroup && genreMarks.find(m => m.name === currentGroup)?.subs.length) {
    if (indexSubs.classList.contains('hidden')) renderSubs(); else show(indexSubs, false);
    return;
  }
  show(indexSubs, false);
  go(Number(b.dataset.index));
  if (e.detail) b.blur();
});
indexSubs.addEventListener('click', e => {
  const b = e.target.closest('button');
  if (!b || libraryGestures.active) return;
  go(Number(b.dataset.index));
  if (e.detail) b.blur();
});

function indexAlbums(albums) {
  const mode = indexMode(), label = modeLabels[mode];
  const next = nextIndexMode();
  const description = next === 'order' ? 'Switch to crate order' : 'Switch to ' + modeLabels[next].toLowerCase() + ' order';
  $('indexModeLabel').textContent = label;
  $('indexMode').ariaDescription = description;
  $('indexMode').setAttribute('aria-label', (mode === 'order' ? 'Crate order' : label + ' order') + '. ' + description);
  show(indexLetters, mode === 'artist' || mode === 'album');
  show(indexGenres, mode === 'genre');
  if (mode === 'genre') return indexByGenre(albums);
  show(indexSubs, false);
  if (mode === 'order') return albums;
  const byTitle = mode === 'album';
  indexLetters.setAttribute('aria-label', label + ' initials');
  const groups = new Map([...letters].map(letter => [letter, []]));
  for (const a of albums) {
    a.initial = albumInitial(a, byTitle);
    groups.get(a.initial).push(a);
  }
  let offset = 0;
  for (const [letter, group] of groups) {
    // Stable sorting keeps the scanner's artist order for albums with the same title.
    if (byTitle) group.sort((a, b) => titleOrder.compare(a.title, b.title));
    const button = letterButtons.get(letter);
    button.setAttribute('aria-label', letter === '#' ? 'Numbers and other initials' : label + 's beginning with ' + letter);
    button.disabled = !group.length;
    button.dataset.index = offset;
    offset += group.length;
  }
  return [...groups.values()].flat();
}

const positionKey = 'muso.libraryPosition';
let savedPosition;
try { savedPosition = JSON.parse(localStorage.getItem(positionKey)); } catch { /* No readable bookmark. */ }
function rememberPosition() {
  if (browsingNew) return;
  const index = Math.round(state.target);
  const id = state.albums[index]?.id;
  if (activeCrate) {
    if (id) cratePosition = { id, index };
    return;
  }
  if (!id || (savedPosition?.id === id && savedPosition.index === index)) return;
  savedPosition = { id, index };
  localStorage.setItem(positionKey, JSON.stringify(savedPosition));
}
window.addEventListener('pagehide', rememberPosition);
document.addEventListener('visibilitychange', () => { if (document.hidden) rememberPosition(); });

function layout() {
  finishCrateMotion();
  state.size = Math.round(clamp(Math.min(window.innerHeight * 0.52, window.innerWidth * 0.36), 220, 640));
  document.documentElement.style.setProperty('--size', state.size + 'px');
  render();
}

function makeSleeve(i) {
  const a = state.albums[i];
  const el = document.createElement('div');
  el.className = 'sleeve';
  el.dataset.index = i;
  if (a.art) {
    const img = new Image();
    img.decoding = 'async';
    img.draggable = false;
    img.src = coverUrl(a, 'thumb');
    img.onerror = () => { img.remove(); fillBlank(el, a); };
    el.appendChild(img);
  } else {
    fillBlank(el, a);
  }
  albumCrates.bindFilingSleeve(a, el);
  return el;
}

function fillBlank(el, a) {
  el.classList.add('blank');
  el.style.setProperty('--blank', blankColor(a.id));
  const ar = document.createElement('div');
  ar.className = 'b-artist';
  ar.textContent = a.artist;
  const ti = document.createElement('div');
  ti.className = 'b-title';
  ti.textContent = a.title;
  el.append(ar, ti);
}

function clearCrate() {
  finishCrateMotion();
  for (const el of pool.values()) el.remove();
  pool.clear();
}

// Horizontal travel from the centre to the next sleeve, shared by layout and input.
const sleeveSpacing = () => state.size * 0.66;

function positionSleeve(el, d, size) {
  const ad = Math.abs(d);
  const s = Math.sign(d);
  const t = Math.min(ad, 1);
  const x = s * (t * (size * 0.66) + Math.max(0, ad - 1) * (size * 0.17));
  const z = -t * size * 0.6 - Math.max(0, ad - 1) * size * 0.05;
  const rot = -s * t * 64;
  el.style.transform = `translate3d(${x.toFixed(1)}px,0,${z.toFixed(1)}px) rotateY(${rot.toFixed(2)}deg)`;
  el.style.zIndex = String(1000 - Math.round(ad * 10));
  el.style.opacity = ad > WINDOW - 1.5 ? String(clamp(WINDOW - 0.5 - ad, 0, 1)) : '1';
  el.style.setProperty('--shade', (Math.min(ad, 5) * 0.16).toFixed(3));
}

function render() {
  const n = state.albums.length;
  if (!n) return;
  const center = Math.round(state.pos);
  const lo = Math.max(0, center - WINDOW);
  const hi = Math.min(n - 1, center + WINDOW);
  for (const [i, el] of pool) {
    if (i < lo || i > hi) { el.remove(); pool.delete(i); }
  }
  for (let i = lo; i <= hi; i++) {
    let el = pool.get(i);
    if (!el) { el = makeSleeve(i); pool.set(i, el); crate.appendChild(el); }
    positionSleeve(el, i - state.pos, state.size);
  }
}

let lastCaption = -1;
let noteTimer, noteRequest = 0, visibleNote = null;
const pendingArticles = new Map();
let articleAlbum;
let hasOpenAIKey = false;
let keyRequest = 0;
async function refreshKeyAvailability() {
  const request = ++keyRequest;
  const available = await invoke('has_openai_key');
  if (request !== keyRequest) return;
  hasOpenAIKey = available;
  show($('rewriteArticle'), hasOpenAIKey);
  if (visibleNote) renderNote();
}
function renderNote() {
  const { album, body } = visibleNote;
  const stage = pendingArticles.get(album.id);
  $('noteTeaser').textContent = body?.split(/\n\s*\n/)[0] || '';
  $('noteAction').textContent = body ? 'Read album article' : 'Write album article with AI';
  show($('noteAction'), !stage && (Boolean(body) || hasOpenAIKey));
  show($('noteProgress'), Boolean(stage));
  $('noteStage').textContent = stage || '';
  show($('notePreview'), Boolean(body || stage || hasOpenAIKey));
}
listen('article-progress', ({ payload: [id, stage] }) => {
  if (!pendingArticles.has(id)) return;
  pendingArticles.set(id, stage);
  if (visibleNote?.album.id === id) renderNote();
});
listen('article-saved', ({ payload: id }) => {
  const album = focused();
  if (album?.id === id && !pendingArticles.has(id)) prepareNote(album);
});
listen('atmos-article-warning', ({ payload }) => toast(payload, 6000));
function prepareNote(album) {
  const request = ++noteRequest;
  clearTimeout(noteTimer);
  visibleNote = null;
  show($('notePreview'), false);
  noteTimer = setTimeout(async () => {
    try {
      const body = await invoke('get_album_note', { albumId: album.id });
      if (request !== noteRequest || focused()?.id !== album.id) return;
      visibleNote = { album, body };
      renderNote();
    } catch (e) { if (request === noteRequest) toast(String(e)); }
  }, 850);
}
$('noteAction').addEventListener('click', async () => {
  if (!visibleNote) return;
  const { album, body } = visibleNote;
  if (body) {
    articleAlbum = album;
    toggleArticleMenu(false);
    $('articleTitle').textContent = album.title;
    $('articleArtist').textContent = album.artist;
    $('articleDetails').textContent = [album.year, album.genre?.trim()].filter(Boolean).join(' · ');
    $('articleBody').replaceChildren(...body.split(/\n\s*\n/).map(text => {
      const p = document.createElement('p'); p.textContent = text;
      if (text.startsWith('Quelle')) p.className = 'source';
      return p;
    }));
    show($('article'));
    $('article').querySelector('.article-content').scrollTop = 0;
    $('article').querySelector('.close').focus({ preventScroll:true });
    return;
  }
  await changeArticle(album, 'write');
});

function toggleArticleMenu(open) {
  show($('articleMenu'), open);
  $('articleMore').setAttribute('aria-expanded', String(open));
  $('deleteArticle').textContent = 'Delete article';
}
$('articleMore').addEventListener('click', () => {
  const open = $('articleMenu').classList.contains('hidden');
  toggleArticleMenu(open);
  if (open) $(hasOpenAIKey ? 'rewriteArticle' : 'deleteArticle').focus();
});
document.addEventListener('click', e => {
  if (!$('articleMenu').classList.contains('hidden') && !e.target.closest('.article-tools')) toggleArticleMenu(false);
});
$('article').addEventListener('keydown', e => {
  if (e.key === 'Escape' && !$('articleMenu').classList.contains('hidden')) {
    e.stopPropagation();
    toggleArticleMenu(false);
    $('articleMore').focus();
  }
});
$('rewriteArticle').addEventListener('click', () => changeArticle(articleAlbum, 'rewrite'));
$('deleteArticle').addEventListener('click', () => {
  if ($('deleteArticle').textContent === 'Delete article') {
    $('deleteArticle').textContent = 'Confirm deletion';
    return;
  }
  changeArticle(articleAlbum, 'delete');
});

async function changeArticle(album, action) {
  const deleting = action === 'delete';
  if (!album || pendingArticles.has(album.id) || (!deleting && !hasOpenAIKey)) return;
  show($('article'), false);
  pendingArticles.set(album.id, deleting ? 'Deleting' : 'Queued');
  if (visibleNote?.album.id === album.id) renderNote();
  try {
    let result = null;
    if (deleting) await invoke('delete_album_note', {albumId:album.id});
    else result = await invoke('generate_album_note', {albumId:album.id, research:researchEnabled(), replace:action === 'rewrite'});
    if (visibleNote?.album.id === album.id) visibleNote.body = result;
  }
  catch (e) { toast(String(e),6000); }
  finally {
    pendingArticles.delete(album.id);
    if (visibleNote?.album.id === album.id) renderNote();
    else if (focused()?.id === album.id) prepareNote(album);
  }
}
for (const [button, importing] of [['exportArchive',false],['importArchive',true]]) {
  $(button).addEventListener('click', async () => {
    $('exportArchive').disabled = $('importArchive').disabled = true;
    $('archiveStatus').textContent = importing ? 'Importing…' : 'Exporting…';
    try {
      const result = await invoke('transfer_library_art', {importing});
      $('archiveStatus').textContent = result ? (importing ? 'Imported' : 'Exported') + ' · ' + result.notes + ' articles · ' + result.images + ' images' + (result.genres ? ' · ' + result.genres + ' album genres' : '') + (result.skipped ? ' · ' + result.skipped + ' unmatched albums' : '') : '';
      if (result && importing) await loadAlbums();
    } catch(e) { $('archiveStatus').textContent = String(e); }
    finally { $('exportArchive').disabled = $('importArchive').disabled = false; }
  });
}
function updateCaption(force) {
  const n = state.albums.length;
  if (!n) {
    clearTimeout(noteTimer);
    ++noteRequest;
    visibleNote = null;
    show($('notePreview'), false);
    $('capArtist').textContent = '';
    $('capTitle').textContent = activeCrate ? (activeCrate.albums.length ? 'No available records in this crate' : 'This crate is empty') : '';
    $('counter').textContent = '0000 / 0000';
    letterButtons.get(currentLetter)?.removeAttribute('aria-current');
    currentLetter = undefined;
    return;
  }
  const i = clamp(Math.round(state.pos), 0, n - 1);
  if (i === lastCaption && !force) return;
  lastCaption = i;
  const a = state.albums[i];
  prepareNote(a);
  const initial = browsingNew || indexMode() === 'order' ? null : a.initial;
  if (currentLetter !== initial) {
    const previous = letterButtons.get(currentLetter);
    if (previous) {
      previous.removeAttribute('aria-current');
      previous.tabIndex = -1;
    }
    const button = letterButtons.get(initial);
    if (button) button.setAttribute('aria-current', 'true');
    const tabStop = button || [...letterButtons.values()].find(b => !b.disabled);
    indexLetters.querySelector('button[tabindex="0"]')?.setAttribute('tabindex', '-1');
    if (tabStop) tabStop.tabIndex = 0;
    currentLetter = initial;
  }
  $('capArtist').textContent = a.artist;
  const title = $('capTitle');
  title.textContent = a.title;
  markGenre(a);
  const genre = a.genreName ?? a.genre?.trim();
  const details = [a.year, genre].filter(Boolean);
  if (details.length) {
    const meta = document.createElement('span');
    meta.className = 'album-meta';
    meta.textContent = details.join(' · ');
    if (a.genreTag && genre && a.genreTag.trim().toLowerCase() !== genre.toLowerCase()) meta.ariaDescription = 'Tag in the file: ' + a.genreTag;
    title.append(' ', meta);
  }
  $('counter').textContent = `${String(i + 1).padStart(4, '0')} / ${String(n).padStart(4, '0')}`;
}

const defaultSettleTime = 95; // Exponential decay time in milliseconds.
let frame = 0, lastFrame = 0, settleTime = defaultSettleTime;
function tick(now) {
  frame = 0;
  const diff = state.target - state.pos;
  const elapsed = Math.max(0, now - lastFrame);
  state.pos = Math.abs(diff) * sleeveSpacing() < 0.25
    ? state.target : state.pos + diff * -Math.expm1(-elapsed / settleTime);
  render();
  updateCaption();
  if (state.pos !== state.target) {
    lastFrame = now;
    frame = requestAnimationFrame(tick);
  } else if (!libraryGestures.active) {
    crate.dispatchEvent(new Event('librarysettled'));
  }
}
function kick() {
  if (frame) return;
  lastFrame = performance.now();
  frame = requestAnimationFrame(tick);
}
function go(index, time = defaultSettleTime) {
  finishCrateMotion();
  const n = state.albums.length;
  if (!n) return;
  const t = clamp(Math.round(index), 0, n - 1);
  // Long jumps (search) start just before the destination so we don't stream thousands of sleeves.
  if (Math.abs(t - state.pos) > 24) state.pos = t - Math.sign(t - state.pos) * 6;
  state.target = t;
  settleTime = time;
  rememberPosition();
  kick();
}
const focused = () => state.albums[clamp(Math.round(state.target), 0, state.albums.length - 1)];

// wheel / trackpad
let wheelAcc = 0;
crate.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (state.view !== 'library' || libraryGestures.active || !state.albums.length) return;
  finishCrateMotion();
  const raw = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
  const d = e.deltaMode === 1 ? raw * 40 : raw;
  wheelAcc += d;
  const step = 70;
  settleTime = defaultSettleTime;
  while (Math.abs(wheelAcc) >= step) {
    const dir = Math.sign(wheelAcc);
    state.target = clamp(state.target + dir, 0, state.albums.length - 1);
    wheelAcc -= dir * step;
  }
  rememberPosition();
  kick();
}, { passive: false });

// drag / swipe
let albumsById = new Map();
function finishCrateMotion() {
  if (!crateMotion) return;
  for (const flight of crateMotion.flights) flight.cancel();
  crateMotion.layer?.remove();
  crateMotion = null;
}

function flyCrateSleeves(records, origin, layer = null) {
  const closing = !!layer;
  const first = records[0].sleeve;
  const stage = first.parentElement.getBoundingClientRect();
  const size = first.offsetWidth;
  const anchorX = stage.left + first.offsetLeft + size / 2;
  const anchorY = stage.top + first.offsetTop + size / 2;
  const flights = [];
  const duration = closing ? 500 : 620;
  records.forEach(({ sleeve, id, from }, i) => {
    const card = origin.cards.get(id);
    const rect = card?.rect ?? origin.rect;
    const x = rect.left + rect.width / 2 - anchorX;
    const y = rect.top + rect.height / 2 - anchorY;
    const scale = rect.width / size;
    const tilt = card?.tilt ?? 0;
    const slot = new DOMMatrixReadOnly(sleeve.style.transform);
    const packed = `translate3d(${x}px,${y}px,0) rotateY(0deg) rotateZ(${tilt}deg) scale(${scale})`;
    const lifted = `translate3d(${x + (slot.m41 - x) * .22}px,${y * .55}px,0) rotateY(0deg) rotateZ(${tilt + Math.sign(slot.m41 - x) * 5}deg) scale(${scale + (1 - scale) * .24})`;
    const ends = closing ? [from, packed] : [packed, sleeve.style.transform];
    const delay = Math.min(closing ? records.length - 1 - i : i, 8) * 30;
    flights.push(sleeve.animate([
      { transform: ends[0] }, { transform: lifted, offset: closing ? .68 : .32 }, { transform: ends[1] },
    ], {
      duration, delay,
      easing: closing ? 'cubic-bezier(.36,.04,.28,1)' : 'cubic-bezier(.2,.64,.24,1)', fill: 'both',
    }));
    if (card) {
      const handoff = closing ? [{ opacity: 0 }, { opacity: 1 }] : [{ opacity: 1 }, { opacity: 0 }];
      flights.push(card.node.animate(handoff, { duration: 75, delay: closing ? delay + duration - 75 : delay, fill: 'both' }));
    }
  });
  if (closing) {
    flights.push(crate.animate([{ opacity: 0 }, { opacity: 0, offset: .55 }, { opacity: 1 }],
      { duration: 500 + Math.min(records.length - 1, 8) * 30, fill: 'both' }));
  }
  const motion = crateMotion = { layer, flights };
  Promise.allSettled(flights.map(f => f.finished)).then(() => {
    if (crateMotion === motion) finishCrateMotion();
  });
}

function crateFlightRecords() {
  if (reducedMotion.matches || document.hidden) return [];
  return [...pool].filter(([, sleeve]) => {
    const r = sleeve.getBoundingClientRect();
    return r.right > 0 && r.left < innerWidth && Number(sleeve.style.opacity) > 0;
  }).map(([i, sleeve]) => ({ sleeve, id: state.albums[i].id, from: getComputedStyle(sleeve).transform }));
}

function browseCrate(list) {
  if (!activeCrate && list) libraryReturn = { album: focused(), index: state.target, recent: browsingNew };
  activeCrate = list;
  crateMode = 'order';
  cratePosition = null;
  if (list) localStorage.setItem(crateKey, list.id);
  else localStorage.removeItem(crateKey);
  closeSearch();
  browsingNew = !list && !!libraryReturn?.recent;
  refreshRecent();
  if (!recentAlbums.length) browsingNew = false;
  const previous = list ? null : libraryReturn?.album ?? albumsById.get(savedPosition?.id);
  applyLibraryOrder(previous, list ? 0 : libraryReturn?.index ?? savedPosition?.index);
  if (!list) libraryReturn = null;
}
$('closeLibraryCrate').addEventListener('click', () => {
  if (!activeCrate) return;
  const records = crateFlightRecords();
  let layer, origin;
  if (records.length) {
    origin = albumCrates.returnOrigin(activeCrate);
    layer = document.createElement('div');
    layer.className = 'returning-sleeves';
    layer.setAttribute('aria-hidden', 'true');
    crate.after(layer);
    for (const { sleeve } of records) {
      pool.delete(Number(sleeve.dataset.index));
      layer.append(sleeve);
    }
  }
  browseCrate(null);
  if (layer) flyCrateSleeves(records, origin, layer);
  crate.focus({ preventScroll: true });
});
const albumCrates = createAlbumCrates({
  invoke, lookup: id => albumsById.get(id), toast, coverUrl, blankColor,
  browse: (list, origin) => {
    browseCrate(list);
    const records = crateFlightRecords();
    if (records.length) flyCrateSleeves(records, origin);
    (state.albums.length ? crate : $('closeLibraryCrate')).focus({ preventScroll: true });
  },
  changed: lists => {
    const list = lists.find(c => String(c.id) === localStorage.getItem(crateKey));
    if (activeCrate && !list) browseCrate(null);
    else if (list && JSON.stringify(list) !== JSON.stringify(activeCrate)) {
      activeCrate = list;
      if (albumsById.size) { refreshRecent(); applyLibraryOrder(); }
    }
  },
  play: albums => {
    leaveCrates();
    const [album, ...remaining] = albums;
    if (state.playing && !state.playing.ended && !state.playing.paused) return openAlbum(album, remaining);
    return startAlbum(album, remaining);
  },
  back: () => leaveCrates(),
});
$('editLibraryCrate').addEventListener('click', () => {
  if (activeCrate) openCrates(activeCrate.id);
});
function openCrates(editId) {
  if (state.view !== 'library') return;
  closeSearch();
  state.view = 'crates';
  show($('library'), false);
  syncMini();
  albumCrates.open(editId);
}
function leaveCrates() {
  if (state.view !== 'crates') return;
  const editing = $('cratesPage').classList.contains('direct-edit');
  albumCrates.close();
  showLibrary();
  if (editing) (activeCrate ? $('editLibraryCrate') : crate).focus({ preventScroll: true });
}
const swipeDirection = $('librarySwipeDirection');
swipeDirection.value = localStorage.getItem('muso.librarySwipeDirection') === 'follow' ? 'follow' : 'opposite';
swipeDirection.addEventListener('change', () => localStorage.setItem('muso.librarySwipeDirection', swipeDirection.value));

const libraryGestures = bindSleeveGestures(crate, state, {
  spacing: sleeveSpacing,
  lastIndex: () => state.albums.length - 1,
  hold: () => { finishCrateMotion(); wheelAcc = 0; },
  render: kick,
  navigate: go,
  activate: sleeve => openLibraryAlbum(state.albums[Number(sleeve.dataset.index)]),
  filing: {
    begin: (sleeve, x, y) => albumCrates.begin(state.albums[Number(sleeve.dataset.index)], sleeve, x, y),
    drag: (x, y) => albumCrates.drag(x, y),
    end: (x, y, cancelled) => albumCrates.end(x, y, cancelled),
  },
});

/* ---------------- library loading ---------------- */

let libraryRequest = 0;
async function loadAlbums() {
  const request = ++libraryRequest;
  const [albums, genres] = await Promise.all([
    invoke('get_albums'),
    invoke('genre_state').catch(e => { console.warn('Muso genres', e); return null; }),
  ]);
  // Replacing sleeves during a drag or glide would round the live position and detach the grabbed cover.
  if (libraryGestures.active || state.pos !== state.target) {
    await new Promise(resolve => crate.addEventListener('librarysettled', resolve, { once: true }));
  }
  if (request !== libraryRequest) return;
  if (genres) genreInfo = genres;
  annotateGenres(albums);
  const byId = new Map(albums.map(a => [a.id, a]));
  albumsById = byId;
  const represented = new Set();
  for (const a of albums) {
    const artists = (a.members || []).map(id => {
      represented.add(id);
      return byId.get(id)?.artist || '';
    });
    a.search = [a.artist, a.title, a.year, ...artists].join(' ').toLowerCase();
  }
  // Show guest tracks on their principal artist's complete release in both browsing modes.
  // Keep individual artist entries for compilations without a principal artist.
  const led = albums.filter(a => a.members?.length > 1 && !['various artists', 'unknown artist'].includes(a.artist.toLowerCase()));
  const ledIds = new Set(led.map(a => a.id));
  const groupedMembers = new Set(led.flatMap(a => a.members));
  const compilationMembers = new Set(albums.filter(a => a.members?.length > 1 && !ledIds.has(a.id)).flatMap(a => a.members));
  artistAlbums = albums.filter(a => {
    if (ledIds.has(a.id)) return true;
    if (a.members?.length) return a.members.length === 1 && groupedMembers.has(a.members[0]) && !compilationMembers.has(a.members[0]);
    return !groupedMembers.has(a.id) || compilationMembers.has(a.id);
  });
  titleAlbums = albums.filter(a => !represented.has(a.id));
  albumCrates.renderDock();
  refreshRecent();
  applyLibraryOrder();
}
function refreshRecent() {
  clearTimeout(expiryTimer);
  const now = Date.now();
  recentAlbums = scopedAlbums(titleAlbums).filter(a => a.first_seen * 1000 + recentWindow > now)
    .sort((a, b) => b.first_seen - a.first_seen || titleOrder.compare(a.title, b.title));
  newButton.disabled = !recentAlbums.length;
  if (recentAlbums.length) {
    const delay = recentAlbums.at(-1).first_seen * 1000 + recentWindow - now;
    expiryTimer = setTimeout(expireRecent, Math.min(delay, 2147483647));
  }
}
function expireRecent() {
  if (libraryGestures.active) { expiryTimer = setTimeout(expireRecent, 500); return; }
  refreshRecent();
  if (browsingNew) applyLibraryOrder();
}
function leaveNew() {
  browsingNew = false;
  const position = activeCrate ? cratePosition : savedPosition;
  applyLibraryOrder(albumsById.get(position?.id) ?? null, position?.index);
}
newButton.addEventListener('click', () => {
  if (libraryGestures.active) return;
  closeSearch();
  if (browsingNew) return leaveNew();
  refreshRecent();
  if (!recentAlbums.length) return;
  rememberPosition();
  browsingNew = true;
  applyLibraryOrder(recentAlbums[0]);
});
function applyLibraryOrder(previous = focused(), fallback = state.albums.length ? Math.round(state.target) : (activeCrate ? 0 : savedPosition?.index)) {
  if (browsingNew && !recentAlbums.length) return leaveNew();
  const keepId = previous?.id ?? (activeCrate ? cratePosition?.id : savedPosition?.id);
  const ordered = indexAlbums(scopedAlbums(modeAlbums()));
  $('libraryCrateName').textContent = activeCrate?.name ?? '';
  show($('libraryCrateHeading'), !!activeCrate);
  show($('editLibraryCrate'), !!activeCrate);
  state.albums = browsingNew ? recentAlbums : ordered;
  newButton.setAttribute('aria-pressed', String(browsingNew));
  clearCrate();
  lastCaption = -1;
  let idx = keepId ? state.albums.findIndex((a) => a.id === keepId) : -1;
  if (idx < 0 && keepId) idx = state.albums.findIndex(a => a.members?.includes(keepId));
  if (idx < 0 && previous?.members) {
    if (previous.members.includes(preferredMember)) idx = state.albums.findIndex(a => a.id === preferredMember);
    if (idx < 0) idx = state.albums.findIndex(a => previous.members.includes(a.id));
  }
  if (idx < 0) idx = clamp(Number.isSafeInteger(fallback) ? fallback : 0, 0, Math.max(0, state.albums.length - 1));
  state.pos = state.target = idx;
  rememberPosition();
  render();
  updateCaption(true);
}
$('indexMode').addEventListener('click', () => {
  if (libraryGestures.active) return;
  closeSearch();
  if (browsingNew) leaveNew();
  if (indexMode() === 'artist') preferredMember = focused()?.id;
  if (activeCrate) crateMode = nextIndexMode();
  else {
    libraryMode = nextIndexMode();
    localStorage.setItem('muso.indexMode', libraryMode);
  }
  applyLibraryOrder();
});

function applyGenreState(s) {
  genreInfo = s;
  annotateGenres([...albumsById.values()]);
  if (state.view !== 'onboard') applyLibraryOrder();
  if (state.viewAlbum) fillAlbumGenre(state.viewAlbum);
}
const genresView = createGenres({
  invoke, listen, toast, coverUrl, blankColor,
  lookup: id => albumsById.get(id),
  records: () => titleAlbums,
  onState: applyGenreState,
  back: () => leaveGenres(),
});
function openGenres(opts) {
  if (state.view !== 'library') return;
  closeSearch();
  show($('ai'), false);
  show($('genreHint'), false);
  state.view = 'genres';
  show($('library'), false);
  syncMini();
  genresView.open(opts);
}
function leaveGenres() {
  if (state.view !== 'genres') return;
  genresView.close();
  showLibrary();
}
$('openGenres').addEventListener('click', () => openGenres());
$('genreHint').addEventListener('click', () => openGenres({ tab: 'sort', buckets: [UNSORTED] }));
const unsortedIds = () => genreInfo.genres.length ? new Set(titleAlbums.filter(a => !a.genreName).map(a => a.id)) : new Set();
function announceUnsorted(before) {
  const fresh = [...unsortedIds()].filter(id => !before.has(id)).length;
  if (!fresh) return;
  $('genreHint').textContent = `${fresh} new album${fresh === 1 ? '' : 's'} without a genre · Sort`;
  show($('genreHint'));
}

// Album menu: your own genre for this record.
function fillAlbumGenre(a) {
  const select = $('albumGenre');
  const g = genreInfo.albums[a.id];
  const own = g?.origin === 'user' ? g.genre : '';
  select.replaceChildren(new Option(own ? 'Automatic (tags & AI)' : 'Automatic · ' + (g?.genre || 'unsorted'), ''));
  const tops = genreInfo.genres.filter(x => !x.parent);
  for (const t of tops) {
    select.append(new Option(t.name, t.name, false, t.name === own));
    for (const k of genreInfo.genres.filter(x => x.parent === t.name)) select.append(new Option('\u2003' + k.name, k.name, false, k.name === own));
  }
  if (own && !genreInfo.genres.some(x => x.name === own)) select.append(new Option(own, own, false, true));
  select.append(new Option('New genre…', '__new'));
  select.value = own;
  show($('albumGenreInput'), false);
}
async function chooseAlbumGenre(genre) {
  const a = state.viewAlbum;
  if (!a) return;
  try {
    applyGenreState(await invoke('set_album_genre', { albumId: a.id, genre: genre || null }));
    toast(genre ? 'Genre set to ' + genre : 'Genre follows tags and AI again');
  } catch (e) { toast(String(e), 5000); fillAlbumGenre(a); }
}
$('albumGenre').addEventListener('change', e => {
  if (e.target.value === '__new') {
    const input = $('albumGenreInput');
    input.value = '';
    show(input);
    input.focus();
    return;
  }
  chooseAlbumGenre(e.target.value);
});
$('albumGenreInput').addEventListener('keydown', e => {
  if (e.key === 'Enter' && e.target.value.trim()) { e.preventDefault(); chooseAlbumGenre(e.target.value.trim()); }
  else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (state.viewAlbum) fillAlbumGenre(state.viewAlbum); }
});

async function boot() {
  refreshKeyAvailability().catch(e => console.warn('Muso API key status', e));
  layout();
  await albumCrates.load();
  const folders = await invoke('get_folders');
  if (!folders.length) {
    showOnboard();
    return;
  }
  await loadAlbums();
  if (albumsById.size) showLibrary();
  else { showOnboard('Looking for music…'); }
  invoke('scan_library'); // quiet incremental refresh
}

function showOnboard(status = '') {
  state.view = 'onboard';
  show($('onboard'));
  show($('library'), false);
  show($('menuBtn'), false);
  $('scanStatus').textContent = status;
}
function showLibrary() {
  state.view = 'library';
  show($('onboard'), false);
  show($('library'));
  show($('menuBtn'));
  layout();
  updateCaption(true);
  syncMini();
}

$('chooseBtn').addEventListener('click', async () => {
  const folders = await invoke('pick_folders');
  if (folders.length) {
    $('scanStatus').textContent = 'Opening the crate…';
    invoke('scan_library');
  }
});

listen('scan-progress', ({ payload }) => {
  const msg = `Digging · ${payload.files.toLocaleString()} files`;
  if (state.view === 'onboard') $('scanStatus').textContent = msg;
  else $('libStatus').textContent = msg;
});
listen('scan-done', async ({ payload }) => {
  $('libStatus').textContent = '';
  if (payload.error) toast(payload.error, 5000);
  const unsortedBefore = unsortedIds();
  await loadAlbums();
  announceUnsorted(unsortedBefore);
  if (albumsById.size) {
    if (state.view === 'onboard') showLibrary();
  } else if (state.view !== 'album') {
    showOnboard();
    $('onboardText').textContent = 'No music found yet. Choose another folder?';
  }
});

/* ---------------- menu overlay ---------------- */

window.__TAURI__.app.getVersion()
  .then(version => { $('menuVersion').textContent = `Muso v${version}`; })
  .catch(error => console.warn('Could not read Muso version', error));

let availableUpdate;
$('menuVersion').addEventListener('click', async () => {
  const button = $('menuVersion');
  const status = $('updateStatus');
  button.disabled = true;
  try {
    if (!availableUpdate) {
      status.textContent = 'Checking for updates…';
      availableUpdate = await window.__TAURI__.updater.check({timeout: 20000});
      status.textContent = availableUpdate ? 'Update available' : 'Muso is up to date.';
      if (availableUpdate) {
        button.textContent = `Install v${availableUpdate.version}`;
        button.ariaDescription = 'Download and install update';
      }
    } else {
      let received = 0, total = 0;
      status.textContent = 'Downloading update…';
      await availableUpdate.downloadAndInstall(({event, data}) => {
        if (event === 'Started') total = data.contentLength || 0;
        if (event === 'Progress') {
          received += data.chunkLength;
          if (total) status.textContent = `Downloading… ${Math.floor(100 * received / total)}%`;
        }
        if (event === 'Finished') status.textContent = 'Installing update…';
      });
      await availableUpdate.close();
      availableUpdate = null;
      status.textContent = 'Restart Muso to use the update.';
      button.textContent = 'Update installed';
      return;
    }
  } catch (e) {
    status.textContent = availableUpdate ? 'Installation failed. Click to retry.' : 'Update check failed. Click to retry.';
    console.warn('Muso update', e);
  }
  button.disabled = false;
});

function openMenu() {
  show($('menu'));
  playerWindow.isFullscreen().then(showFullscreenAction).catch(e => toast(String(e)));
}
let menuSelection = 0;
function closeMenu() {
  ++menuSelection;
  show($('menu'), false);
}
$('menuBtn').addEventListener('click', openMenu);
$('menu').querySelector('.menu-scrim').addEventListener('click', closeMenu);
$('menu').querySelectorAll('[data-action]').forEach((b) =>
  b.addEventListener('click', async () => {
    const act = b.dataset.action;
    if (act === 'settings' || act === 'ai') {
      const selection = ++menuSelection;
      try {
        const [folders] = await Promise.all([act === 'settings' ? invoke('get_folders') : null, loadKey()]);
        if (selection !== menuSelection) return;
        if (act === 'settings') renderFolders(folders);
        else prepareAi();
        show($(act));
        $(act).querySelector('.modal-scroll').scrollTop = 0;
        $(act).querySelector('.settings-dismiss').focus({ preventScroll: true });
      } catch (error) {
        if (selection === menuSelection) toast(String(error));
        return;
      }
    }
    closeMenu();
    if (act === 'search') openSearch();
    if (act === 'crates') openCrates();
    if (act === 'eq') openEQ(false);
    if (act === 'images') openImagesDialog();
    if (act === 'presenter') openTvDialog();
    if (act === 'audio-output') heosOutput.open();
    if (act === 'fullscreen') toggleFullscreen();
    if (act === 'refresh') {
      invoke('scan_library').then((started) => toast(started ? 'Refreshing library…' : 'Already digging…'));
    }
  })
);

/* ---------------- search ---------------- */

let results = [];
let activeResult = 0;
function openSearch(initial = '') {
  if (state.view !== 'library') return;
  show($('search'));
  const input = $('searchInput');
  input.value = initial;
  input.focus();
  runSearch();
}
function closeSearch() {
  show($('search'), false);
  $('searchResults').innerHTML = '';
}
function runSearch() {
  const value = $('searchInput').value;
  $('clearSearch').hidden = value.length === 0;
  const q = value.trim().toLowerCase();
  const box = $('searchResults');
  box.innerHTML = '';
  results = [];
  activeResult = 0;
  if (!q) return;
  const words = q.split(/\s+/);
  for (let i = 0; i < state.albums.length && results.length < 6; i++) {
    const s = state.albums[i].search;
    if (words.every((w) => s.includes(w))) results.push(i);
  }
  results.forEach((i, k) => {
    const a = state.albums[i];
    const b = document.createElement('button');
    if (k === 0) b.classList.add('active');
    const ar = document.createElement('span');
    ar.className = 'a';
    ar.textContent = a.artist;
    b.append(ar, document.createTextNode(a.title));
    b.addEventListener('click', () => { go(i); closeSearch(); });
    box.appendChild(b);
  });
  if (results.length) go(results[0]);
}
function setActive(k) {
  if (!results.length) return;
  activeResult = (k + results.length) % results.length;
  [...$('searchResults').children].forEach((b, j) => b.classList.toggle('active', j === activeResult));
  go(results[activeResult]);
}
$('searchInput').addEventListener('input', runSearch);
$('clearSearch').addEventListener('click', () => {
  $('searchInput').value = '';
  runSearch();
  $('searchInput').focus();
});
$('searchInput').addEventListener('keydown', (e) => {
  if (e.key === 'Escape') { closeSearch(); e.stopPropagation(); }
  else if (e.key === 'Enter') { closeSearch(); e.stopPropagation(); }
  else if (e.key === 'ArrowDown') { setActive(activeResult + 1); e.preventDefault(); }
  else if (e.key === 'ArrowUp') { setActive(activeResult - 1); e.preventDefault(); }
  e.stopPropagation();
});

/* ---------------- settings / ai ---------------- */

async function renderFolders(list) {
  const ul = $('folderList');
  ul.innerHTML = '';
  for (const f of list) {
    const li = document.createElement('li');
    const s = document.createElement('span');
    s.textContent = f;
    s.ariaDescription = f;
    const remove = document.createElement('button');
    remove.textContent = 'Remove';
    remove.setAttribute('aria-label', 'Remove music folder ' + f);
    remove.addEventListener('click', async () => {
      renderFolders(await invoke('remove_folder', { path: f }));
      invoke('scan_library');
    });
    li.append(s, remove);
    ul.appendChild(li);
  }
}
async function loadKey() {
  const key = (await invoke('get_setting', { key: 'openai_key' })) || '';
  const fromEnvironment = await invoke('has_environment_key');
  for (const input of [$('keyInput'), $('keyInput2')]) {
    input.value = key;
    input.placeholder = fromEnvironment ? 'Using OPENAI_API_KEY from this computer' : 'Paste API key';
  }
}
async function saveKey(e) {
  const value = e.target.value.trim();
  try {
    await invoke('set_setting', { key: 'openai_key', value });
    await refreshKeyAvailability();
    toast(value ? 'Key saved' : 'Saved key removed');
  } catch (e) { toast(String(e)); }
}
document.querySelectorAll('.openai-keys').forEach((button) => {
  button.addEventListener('click', () => invoke('open_openai_keys').catch((e) => toast(String(e))));
});
$('keyInput').addEventListener('change', saveKey);
$('keyInput2').addEventListener('change', saveKey);

const filterSelect = $('imageFilter');
const rippleToggle = $('pointerRipples');
const strengthInput = $('effectStrength');
const localRefraction = createLinuxRefraction($('atmosPictures'));
const spectrum = createSpectrumView(invoke, listen, localRefraction, () => syncTv());
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
reducedMotion.addEventListener('change', e => { if (e.matches) finishCrateMotion(); });
let waveFrame = 0;
let wave;
filterSelect.value = localStorage.getItem('muso.imageFilter') || 'light';
if (!filterSelect.value) filterSelect.value = 'light';
rippleToggle.checked = localStorage.getItem('muso.pointerRipples') !== 'off';
strengthInput.value = localStorage.getItem('muso.effectStrength') ?? '40';
function applyEffects() {
  $('album').dataset.filter = filterSelect.value;
  $('album').classList.toggle('zero-effect', Number(strengthInput.value) === 0);
  $('album').style.setProperty('--effect-strength', Number(strengthInput.value) / 100);
  if (!rippleToggle.checked || Number(strengthInput.value) === 0) stopWave();
}
filterSelect.addEventListener('change', () => {
  localStorage.setItem('muso.imageFilter', filterSelect.value);
  applyEffects();
});
rippleToggle.addEventListener('change', () => {
  localStorage.setItem('muso.pointerRipples', rippleToggle.checked ? 'on' : 'off');
  applyEffects();
});
strengthInput.addEventListener('input', () => {
  localStorage.setItem('muso.effectStrength', strengthInput.value);
  applyEffects();
});
applyEffects();

function stopWave() {
  cancelAnimationFrame(waveFrame);
  waveFrame = 0;
  wave = null;
  localRefraction?.clear('pointer');
  $('atmosPictures').classList.remove('refracting');
}
function drawWave(now) {
  const age = Math.max(0, (now - wave.lastContact) / 1250);
  if (age >= 1 || document.hidden || reducedMotion.matches) { stopWave(); return; }
  // The displacement field samples the existing image. Neutral grey leaves pixels in place.
  const follow = 1 - Math.exp(-Math.max(0, now - wave.lastFrame) / 55);
  wave.x += (wave.targetX - wave.x) * follow;
  wave.y += (wave.targetY - wave.y) * follow;
  wave.lastFrame = now;
  const t = (now - wave.start) / 185 + wave.phase;
  const unrest = (1 - age) * (.35 + .65 * wave.motion);
  const diameter = 155 + 220 * age;
  const width = diameter * (1 + unrest * (.14 * Math.sin(t) + .09 * Math.sin(2.73 * t)));
  const height = diameter * (1 + unrest * (.12 * Math.cos(1.47 * t) - .07 * Math.sin(2.21 * t)));
  if (localRefraction) {
    const bounds = $('atmosPictures').getBoundingClientRect();
    localRefraction.pointer(bounds.left + wave.x, bounds.top + wave.y, width, height/width, t,
      (50 + 9*unrest) * Number(strengthInput.value)/100 * (1-age)**2);
    waveFrame = requestAnimationFrame(drawWave);
    return;
  }
  const field = $('waveField');
  field.setAttribute('x', wave.x - width / 2 + unrest * 9 * Math.sin(1.83 * t));
  field.setAttribute('y', wave.y - height / 2 + unrest * 7 * Math.cos(2.37 * t));
  field.setAttribute('width', width);
  field.setAttribute('height', height);
  $('waveDepth').setAttribute('scale', (50 + 9 * unrest) * (Number(strengthInput.value) / 100) * (1 - age) ** 2);
  $('atmosPictures').classList.add('refracting');
  waveFrame = requestAnimationFrame(drawWave);
}
function pointerRipple(e) {
  const album = $('album');
  if (!rippleToggle.checked || reducedMotion.matches || Number(strengthInput.value) === 0 ||
      !album.classList.contains('showing-atmos') || e.target.closest('button, .popover, .controls, .al-track, #playerAudio')) return;
  const image = $('atmosPictures').querySelector('img.on');
  if (!image?.complete || !image.naturalWidth) return;
  const bounds = $('atmosPictures').getBoundingClientRect();
  const x = e.clientX - bounds.left, y = e.clientY - bounds.top;
  const now = performance.now();
  if (!wave) wave = { x, y, start: now, lastFrame: now, phase: Math.random() * Math.PI * 2, motion: 0, vx: 0, vy: 0 };
  else {
    const dt = Math.max(8, now - wave.lastContact);
    const vx = (x - wave.targetX) / dt, vy = (y - wave.targetY) / dt;
    const impulse = .5 * Math.hypot(vx, vy) + .8 * Math.hypot(vx - wave.vx, vy - wave.vy);
    wave.motion = .6 * wave.motion + .4 * Math.min(1, impulse);
    wave.vx = vx; wave.vy = vy;
  }
  Object.assign(wave, { targetX: x, targetY: y, lastContact: now });
  if (!waveFrame) waveFrame = requestAnimationFrame(drawWave);
}
for (const event of ['pointermove', 'pointerdown']) $('album').addEventListener(event, pointerRipple);
document.addEventListener('visibilitychange', () => { if (document.hidden) stopWave(); });
$('addFolderBtn').addEventListener('click', async () => {
  renderFolders(await invoke('pick_folders'));
  invoke('scan_library');
});

let coverSearchRunning = false;
let stopCoverSearch = false;
let currentCoverSearch = null;
const coverSearchResults = new Map();
function renderCoverSearchStatus() {
  const id = state.viewAlbum?.id;
  const pending = currentCoverSearch?.single && currentCoverSearch.id === id;
  const live = $('coverSearchStatus');
  live.hidden = !pending;
  live.textContent = pending ? currentCoverSearch.message : '';
  const result = $('coverSearchResult');
  result.textContent = coverSearchResults.get(id) || '';
  result.hidden = pending || !result.textContent;
}
listen('cover-search-progress', ({ payload }) => {
  if (currentCoverSearch?.id !== payload.albumId) return;
  if (payload.source) currentCoverSearch.sources.add(payload.source);
  currentCoverSearch.message = payload.message;
  $('coverProgress').textContent = currentCoverSearch.label + ' · ' + payload.message;
  renderCoverSearchStatus();
});

function applyCover(id, file) {
  const updated = new Set();
  for (const a of [...albumsById.values(), state.viewAlbum, state.playing?.album]) {
    const inherited = a?.members?.includes(id) && (a.cover_file === id + '.jpg' || a.cover_file?.startsWith(id + '_'));
    if (a && (a.id === id || inherited)) {
      Object.assign(a, { art: true, cover_file: file });
      updated.add(a.id);
    }
  }
  for (const [i, sleeve] of pool) {
    if (updated.has(state.albums[i]?.id)) { sleeve.remove(); pool.delete(i); }
  }
  render();
  albumCrates.renderDock();
  if (updated.has(state.viewAlbum?.id)) {
    for (const el of [$('coverImg'), $('bgArt')]) {
      el.src = coverUrl(state.viewAlbum, 'art');
      show(el);
    }
  }
}

function confirmCover(album, title, text, action, choices = []) {
  const dialog = $('coverConfirm');
  $('coverConfirmTitle').textContent = title;
  $('coverConfirmAlbum').textContent = [album.artist, album.title].filter(Boolean).join(' — ');
  $('coverConfirmText').textContent = text;
  $('coverConfirmAction').textContent = action;
  $('coverConfirmCancel').textContent = choices.length && album.art ? 'Keep current cover' : 'Cancel';
  const backdrop = $('coverBackdrop');
  backdrop.removeAttribute('src');
  dialog.classList.toggle('reviewing-cover', choices.length > 0);
  const stage = $('coverPreview');
  stage.replaceChildren();
  stage.hidden = !choices.length;
  const list = $('coverChoices');
  list.replaceChildren();
  let selection = choices.length === 1 ? choices[0] : null;
  $('coverConfirmAction').disabled = choices.length > 1;
  function showPreview(choice) {
    const { provider, image, id } = choice.location;
    const preview = new Image();
    preview.alt = choice.label;
    preview.referrerPolicy = 'no-referrer';
    backdrop.removeAttribute('src');
    preview.onload = () => {
      if (dialog.open && preview.parentNode === stage) backdrop.src = preview.currentSrc;
    };
    const archiveFront = `https://coverartarchive.org/release-group/${id}/front`;
    preview.onerror = () => {
      if (preview.parentNode !== stage) return;
      if (provider === 'music_brainz' && preview.src !== archiveFront) {
        preview.src = archiveFront;
        return;
      }
      const message = document.createElement('span');
      message.setAttribute('role', 'status');
      message.textContent = 'Cover preview unavailable';
      preview.replaceWith(message);
    };
    stage.replaceChildren(preview);
    preview.src = provider === 'the_audio_db' ? image : archiveFront + '-1200';
  }
  if (choices.length) showPreview(choices[0]);
  choices.forEach(choice => {
    const details = document.createElement('span');
    details.textContent = choice.label;
    const source = document.createElement('small');
    source.textContent = choice.location.provider === 'the_audio_db' ? 'TheAudioDB' : 'Cover Art Archive · MusicBrainz';
    details.append(source);
    if (choices.length === 1) { list.append(details); return; }
    const row = document.createElement('label');
    const radio = document.createElement('input');
    radio.type = 'radio';
    radio.name = 'cover-choice';
    radio.onchange = () => {
      selection = choice;
      showPreview(choice);
      $('coverConfirmAction').disabled = false;
    };
    const thumb = new Image();
    thumb.alt = '';
    thumb.loading = 'lazy';
    thumb.referrerPolicy = 'no-referrer';
    thumb.onerror = () => { thumb.hidden = true; };
    const { provider, image, id } = choice.location;
    thumb.src = provider === 'the_audio_db' ? image : `https://coverartarchive.org/release-group/${id}/front-250`;
    row.append(radio, thumb, details);
    list.append(row);
  });
  dialog.returnValue = '';
  return new Promise(resolve => {
    dialog.addEventListener('close', () => {
      backdrop.removeAttribute('src');
      resolve(dialog.returnValue === 'accept' && (choices.length ? selection : true));
    }, { once: true });
    dialog.showModal();
  });
}

const writeMp3Covers = $('writeMp3Covers');
writeMp3Covers.checked = localStorage.getItem('muso.writeMp3Covers') === 'on';
writeMp3Covers.addEventListener('change', () => localStorage.setItem('muso.writeMp3Covers', writeMp3Covers.checked ? 'on' : 'off'));
async function findAlbumCover(album, single) {
  const run = step => invoke('upgrade_cover', { albumId: album.id, step, writeMp3Covers: writeMp3Covers.checked });
  let result = await run({ type: 'lookup', offer_ai: single });
  if (!single) return result;
  if (result.status === 'offer_research') {
    const reason = result.reason || 'The cover sources couldn’t find a front cover for this album.';
    const sources = [...currentCoverSearch.sources].join(', ');
    if (state.viewAlbum?.id !== album.id || !await confirmCover(album, 'Identify album with AI?',
      (sources ? 'Checked: ' + sources + '. ' : '') + reason + ' Muso can send the artist, album title, year and up to 12 track titles to OpenAI for one paid web search.',
      'Search with AI')) return { status: 'cancelled' };
    result = await run({ type: 'research' });
  }
  if (result.status !== 'review_covers') return result;
  if (state.viewAlbum?.id !== album.id) return { status: 'cancelled' };
  const multiple = result.choices.length > 1;
  const choice = await confirmCover(album, multiple ? 'Choose a front cover' : 'Use this release?',
    multiple ? 'Select the edition that matches your album.' : 'Use this front cover if it improves your current artwork?',
    'Use cover', result.choices);
  return choice ? run({ type: 'use_release', location: choice.location }) : { status: 'cancelled' };
}

async function findCovers(albums, single = false) {
  if (coverSearchRunning) return;
  coverSearchRunning = true;
  stopCoverSearch = false;
  const button = $('findCovers');
  button.textContent = 'Stop search';
  $('findAlbumCover').disabled = true;
  let checked = 0, updated = 0, ambiguous = 0, failed = 0, consecutiveErrors = 0;
  let lastError = '';
  let tagsWritten = 0, tagsFailed = 0, firstTagError = '';
  const messages = {
    updated: 'Cover updated',
    already_large_enough: 'Front cover found. Keeping your current resolution.',
    no_match: 'No matching cover release found',
    ambiguous: 'Multiple editions found. Choose a cover from the album menu.',
    no_front_cover: 'Release found, but no front cover was available',
    unknown_artist: 'Artist missing from the library entry',
    cancelled: 'Cover search cancelled',
  };
  let resultMessage = '';
  try {
    for (const album of albums) {
      if (stopCoverSearch) break;
      currentCoverSearch = { id: album.id, single, label: `Checking ${checked + 1} of ${albums.length} · ${album.title}`,
        sources: new Set(), message: 'Starting cover search…' };
      renderCoverSearchStatus();
      $('coverProgress').textContent = currentCoverSearch.label;
      try {
        const result = await findAlbumCover(album, single);
        resultMessage = messages[result.status];
        if (result.status === 'updated') {
          applyCover(album.id, result.file);
          updated++;
          resultMessage += ' · ' + result.source.provider;
          tagsWritten += result.tags?.written || 0;
          tagsFailed += result.tags?.failed || 0;
          firstTagError ||= result.tags?.first_error || '';
        }
        if (result.status === 'ambiguous') ambiguous++;
        consecutiveErrors = 0;
      } catch (e) {
        failed++;
        consecutiveErrors++;
        lastError = String(e);
      }
      checked++;
      if (consecutiveErrors >= 3) break;
    }
    let summary = `${checked} of ${albums.length} checked · ${updated} updated`;
    if (ambiguous) summary += ` · ${ambiguous} with multiple editions`;
    const tagSummary = (tagsWritten ? ` · ${tagsWritten} MP3 tag(s) updated` : '') +
      (tagsFailed ? ` · ${tagsFailed} MP3 tag(s) failed. ${firstTagError}` : '');
    if (single && currentCoverSearch) {
      summary = (failed ? lastError : resultMessage) + tagSummary;
      const sources = [...currentCoverSearch.sources];
      const detail = sources.length ? summary + ' · Checked: ' + sources.join(', ') : summary;
      coverSearchResults.set(currentCoverSearch.id, detail);
      summary = detail;
    }
    else summary += (failed ? ` · ${failed} failed. ${lastError}` : '') + tagSummary;
    $('coverProgress').textContent = summary;
    if (single || $('settings').classList.contains('hidden')) toast(summary, 4500);
  } finally {
    currentCoverSearch = null;
    renderCoverSearchStatus();
    coverSearchRunning = false;
    button.disabled = false;
    button.textContent = 'Find better covers';
    $('findAlbumCover').disabled = false;
  }
}
$('findCovers').addEventListener('click', () => {
  if (coverSearchRunning) {
    stopCoverSearch = true;
    $('findCovers').disabled = true;
    $('findCovers').textContent = 'Stopping…';
  } else {
    findCovers(modeAlbums().slice());
  }
});
$('findAlbumCover').addEventListener('click', () => {
  show($('albumMenu'), false);
  if (state.viewAlbum) {
    findCovers([state.viewAlbum], true);
  }
});

const atmosEnabled = () => localStorage.getItem('muso.atmos') !== '0';
const styleNames = {
  cinematic: 'Cinematic',
  dreamscape: 'Dreamscape',
  ambient: 'Ambient Drift',
  tape: 'Tape Decay',
  photogram: 'Silver Photogram',
  cel: 'Midnight Cel',
  ukiyoe: 'Ukiyo-e Woodcut',
  dither: 'Dithered Worlds',
  pixelart: 'Pixel Art',
  postmodern: 'Postmodern Realism',
  surreal: 'Surreal Realism',
  urbanphoto: 'Urban Photography',
  landscapephoto: 'Landscape Photography',
  nouveau: 'Art Nouveau',
  bauhaus: 'Bauhaus Study',
  sumi: 'Sumi-e Ink',
  raytrace: 'Early Raytracing',
  futurism: 'Futurist Motion',
  risograph: 'Risograph Layers',
  symbolism: 'Symbolist Nocturne',
  biomechanical: 'Biomechanical',
  streetart: 'Modern Street Art',
};
function albumStyleOverride(id) {
  const saved = localStorage.getItem('muso.style.' + id);
  return Object.hasOwn(styleNames, saved) ? saved : '';
}
function albumStyle(id) {
  return albumStyleOverride(id) || ($('autoStyle').checked ? 'auto' : 'cinematic');
}
let styleAlbum;
const stylePicker = $('stylePicker');
const styleTriggers = [$('albumStyle'), $('aiStyle')];
const personaPicker = $('personaPicker');
const personaTriggers = [$('albumPersona'), $('aiPersona')];
const personaChoices = new Map([...personaPicker.querySelectorAll('button[value]')].map(button => [button.value, button]));
const artistPersonasEnabled = () => localStorage.getItem('muso.artistPersonas') === '1';

function albumPersonaOverride(id) {
  const saved = localStorage.getItem('muso.persona.' + id);
  return personaChoices.has(saved) ? saved : '';
}
function albumPersona(id) {
  return albumPersonaOverride(id) || (artistPersonasEnabled() ? 'random' : 'none');
}
function showPersona(id) {
  const selected = albumPersonaOverride(id);
  const defaultLabel = artistPersonasEnabled() ? 'Default · Random artist' : 'Default · No persona';
  $('personaDefault').textContent = defaultLabel;
  const label = selected === 'random' || selected === 'none' ? personaChoices.get(selected).textContent : selected;
  for (const trigger of personaTriggers) {
    trigger.disabled = !id;
    trigger.textContent = label || defaultLabel;
  }
  for (const [value, choice] of personaChoices) choice.setAttribute('aria-pressed', String(value === selected));
}
for (const trigger of personaTriggers) trigger.addEventListener('click', () => {
  const album = state.viewAlbum || focused();
  if (!album) return;
  showStyle(album.id);
  trigger.focus({ preventScroll: true });
  personaCrate.open(albumPersonaOverride(album.id));
});
function choosePersona(value) {
  if (!styleAlbum) return;
  const key = 'muso.persona.' + styleAlbum;
  if (value) localStorage.setItem(key, value);
  else localStorage.removeItem(key);
  showPersona(styleAlbum);
  personaCrate.close();
}
const personaCrate = createCardCrate(personaPicker, choosePersona);
personaPicker.querySelector('.persona-modes').addEventListener('click', e => {
  const button = e.target.closest('button');
  if (button) choosePersona(button.value);
});

async function showStyle(id) {
  if (stylePicker.open && styleAlbum !== id) styleCrate.close();
  if (personaPicker.open && styleAlbum !== id) personaCrate.close();
  styleAlbum = id;
  showPersona(id);
  const aiDefault = $('autoStyle').checked;
  const style = albumStyle(id);
  const override = albumStyleOverride(id);
  function display(defaultLabel) {
    $('defaultStyleChoice').querySelector('span').textContent = defaultLabel;
    for (const trigger of styleTriggers) {
      trigger.disabled = !id;
      trigger.textContent = styleNames[override] || defaultLabel;
    }
  }
  display(aiDefault ? 'Default · AI choice' : 'Default · Cinematic');
  $('genLabel').textContent = style === 'auto' ? 'Generate with AI style' : 'Generate ' + styleNames[style] + ' images';
  if (aiDefault && id) {
    const name = await invoke('get_setting', { key: 'atmos_auto_style.' + id });
    if (name && styleAlbum === id && $('autoStyle').checked && override === albumStyleOverride(id)) {
      display('AI choice · ' + name);
    }
  }
}
function chooseStyle(style) {
  styleCrate.close();
  if (style === albumStyleOverride(styleAlbum)) return;
  const key = 'muso.style.' + styleAlbum;
  if (style) localStorage.setItem(key, style);
  else localStorage.removeItem(key);
  showStyle(styleAlbum);
  syncTv();
  if (state.viewAlbum?.id === styleAlbum) startAtmospheres(styleAlbum);
}
const styleCrate = createStyleCrate(stylePicker, styleNames, chooseStyle);
for (const trigger of styleTriggers) trigger.addEventListener('click', () => {
  const album = state.viewAlbum || focused();
  if (!album || stylePicker.open) return;
  showStyle(album.id);
  trigger.focus({ preventScroll: true });
  const selected = albumStyleOverride(album.id);
  $('defaultStyleChoice').setAttribute('aria-pressed', String(!selected));
  styleCrate.open(selected);
});
$('defaultStyleChoice').addEventListener('click', () => chooseStyle(''));
$('autoStyle').checked = localStorage.getItem('muso.autoStyle') === '1';
$('autoStyle').addEventListener('change', () => {
  localStorage.setItem('muso.autoStyle', $('autoStyle').checked ? '1' : '0');
  showStyle((state.viewAlbum || focused())?.id);
  syncTv();
  if (state.viewAlbum) startAtmospheres(state.viewAlbum.id);
});
const generateOnPlay = $('generateOnPlay'), playImageCount = $('playImageCount');
generateOnPlay.checked = localStorage.getItem('muso.generateOnPlay') === '1';
const savedCount = localStorage.getItem('muso.playImageCount');
playImageCount.value = ['3', '4', '5'].includes(savedCount) ? savedCount : '3';
playImageCount.disabled = !generateOnPlay.checked;
generateOnPlay.addEventListener('change', () => {
  localStorage.setItem('muso.generateOnPlay', generateOnPlay.checked ? '1' : '0');
  playImageCount.disabled = !generateOnPlay.checked;
});
playImageCount.addEventListener('change', () => localStorage.setItem('muso.playImageCount', playImageCount.value));
for (const select of [$('imageModel'), $('imageResolution')]) {
  const key = 'muso.' + select.id;
  const saved = localStorage.getItem(key);
  if ([...select.options].some(option => option.value === saved)) select.value = saved;
  select.addEventListener('change', () => localStorage.setItem(key, select.value));
}
function prepareAi() {
  $('atmosToggle').checked = atmosEnabled();
  $('repaintToggle').checked = repaintEnabled();
  $('artistPersonas').checked = artistPersonasEnabled();
  $('researchToggle').checked = researchEnabled();
  showStyle((state.viewAlbum || focused())?.id);
}
const repaintEnabled = () => localStorage.getItem('muso.repaint') === '1';
$('artistPersonas').addEventListener('change', (e) => {
  localStorage.setItem('muso.artistPersonas', e.target.checked ? '1' : '0');
  showPersona(styleAlbum);
});
$('repaintToggle').addEventListener('change', (e) => localStorage.setItem('muso.repaint', e.target.checked ? '1' : '0'));
const researchEnabled = () => localStorage.getItem('muso.research') === '1';
$('researchToggle').addEventListener('change', (e) => localStorage.setItem('muso.research', e.target.checked ? '1' : '0'));
function setAtmosEnabled(enabled) {
  localStorage.setItem('muso.atmos', enabled ? '1' : '0');
  $('atmosToggle').checked = enabled;
  if (state.viewAlbum) startAtmospheres(state.viewAlbum.id, enabled);
}
$('atmosToggle').addEventListener('change', (e) => setAtmosEnabled(e.target.checked));
$('aiGenerateBtn').addEventListener('click', () => {
  const a = state.viewAlbum || focused();
  show($('ai'), false);
  if (a) generate(a);
});

document.querySelectorAll('.modal').forEach((m) => {
  function dismiss() {
    show(m, false);
    if (m.classList.contains('preferences')) $('menuBtn').focus({ preventScroll: true });
  }
  m.addEventListener('click', (e) => { if (e.target === m) dismiss(); });
  m.querySelectorAll('.close').forEach(button => button.addEventListener('click', dismiss));
  if (m.classList.contains('preferences')) {
    const first = m.querySelector('.settings-dismiss'), last = m.querySelector('.modal-actions .close');
    m.addEventListener('keydown', e => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        dismiss();
      } else if (e.key === 'Tab' && (e.shiftKey ? e.target === first : e.target === last)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      }
    });
  }
});

/* ---------------- personal image folders ---------------- */
const personalSets = new Map();
let imageInventory = null, imageBusy = false, classifying = false, stopClassification = false;
const imageMix = $('imageMix');
imageMix.checked = localStorage.getItem('muso.imageMix') === '1';
imageMix.onchange = () => {
  localStorage.setItem('muso.imageMix', imageMix.checked ? '1' : '0');
  refreshPersonalPlayback();
};
$('imageMatch').checked = localStorage.getItem('muso.imageMatch') === '1';
$('imageOnline').checked = localStorage.getItem('muso.imageOnline') === '1';
$('imageOnline').disabled = !$('imageMatch').checked;
function refreshPersonalPlayback() {
  personalSets.clear();
  syncTv(true);
  if (state.viewAlbum) startAtmospheres(state.viewAlbum.id);
}
function renderImageFolders(info) {
  imageInventory = info;
  $('imageFolders').replaceChildren(...info.folders.map(folder => {
    const row = document.createElement('li'), name = document.createElement('span'), remove = document.createElement('button');
    name.textContent = folder;
    remove.textContent = 'Remove';
    remove.className = 'link';
    remove.disabled = imageBusy;
    remove.onclick = () => changeImageFolders('remove_image_folder', {folder});
    row.append(name, remove);
    return row;
  }));
  $('imageAdd').disabled = imageBusy;
  $('imageRefresh').disabled = imageBusy || !info.folders.length;
  $('imageClassify').textContent = classifying
    ? (stopClassification ? 'Stopping…' : 'Stop after current image')
    : (info.pending ? 'Classify ' + info.pending + (info.pending === 1 ? ' image' : ' images') : 'No images awaiting classification');
  $('imageClassify').disabled = classifying ? stopClassification : imageBusy || !info.pending;
  $('imageStatus').textContent = info.total + ' images · ' + (info.total - info.pending - info.unreadable) + ' classified'
    + (info.unreadable ? ' · ' + info.unreadable + ' unreadable' : '');
}
async function openImagesDialog() {
  try {
    renderImageFolders(await invoke('image_inventory'));
    $('imagesDialog').showModal();
  } catch (error) { toast(String(error)); }
}
async function changeImageFolders(command, args) {
  if (imageBusy) return;
  imageBusy = true;
  renderImageFolders(imageInventory);
  $('imageStatus').textContent = 'Updating image folders…';
  let failure;
  try { imageInventory = await invoke(command, args); }
  catch (error) { failure = String(error); }
  finally {
    imageBusy = false;
    try { renderImageFolders(await invoke('image_inventory')); }
    catch (error) { renderImageFolders(imageInventory); failure ||= String(error); }
    refreshPersonalPlayback();
    if (failure) $('imageStatus').textContent = failure;
  }
}
$('imageAdd').onclick = () => changeImageFolders('pick_image_folders');
$('imageRefresh').onclick = () => changeImageFolders('rescan_image_folders');
$('imageMatch').onchange = e => {
  localStorage.setItem('muso.imageMatch', e.target.checked ? '1' : '0');
  $('imageOnline').disabled = !e.target.checked;
  refreshPersonalPlayback();
};
$('imageOnline').onchange = e => {
  localStorage.setItem('muso.imageOnline', e.target.checked ? '1' : '0');
  refreshPersonalPlayback();
};
$('imageClassify').onclick = async () => {
  if (classifying) { stopClassification = true; renderImageFolders(imageInventory); return; }
  if (imageBusy) return;
  imageBusy = classifying = true;
  stopClassification = false;
  const budget = imageInventory.pending;
  renderImageFolders(imageInventory);
  let failure;
  try {
    for (let n = 0; n < budget && !stopClassification && imageInventory.pending; n++) {
      renderImageFolders(await invoke('classify_next_image'));
    }
  } catch (error) { failure = String(error); }
  finally {
    imageBusy = classifying = false;
    renderImageFolders(imageInventory);
    if (failure) $('imageStatus').textContent = failure;
    refreshPersonalPlayback();
  }
};
$('imageDone').onclick = () => $('imagesDialog').close();
for (const dialog of document.querySelectorAll('.library-dialog')) {
  dialog.addEventListener('click', e => {
    if (e.target !== dialog) return;
    const box = dialog.getBoundingClientRect();
    if (e.clientX < box.left || e.clientX > box.right || e.clientY < box.top || e.clientY > box.bottom) dialog.close();
  });
}

const heosOutput = createHeosOutput(invoke, listen, toast, () => {
  playerVolume.close();
  if (state.view === 'album') playerVolume.refresh();
});
const playerVolume = createPlayerVolume(invoke, heosOutput, toast, () => { if (state.view === 'album') poke(); });

/* ---------------- TV display ---------------- */
const tvChannel = new BroadcastChannel('muso-presenter');
let tvActive = false, tvReady = false, tvSet = null;
const librarySaver = createLibrarySaver(() => tvActive && state.view === 'library');
async function savedPaintingSet(albumId, style) {
  for (const candidate of new Set([style, ...Object.keys(styleNames), 'auto'])) {
    const files = await invoke('get_atmospheres', { albumId, style: candidate });
    if (files.length) return albumPaintingSeries(albumId, candidate, files);
  }
  return {style: 'personal', files: await personalPictures(albumId)};
}
async function albumPaintingSeries(albumId, style, paintings) {
  if (!imageMix.checked) return {style, files: paintings};
  let personal;
  try { personal = await personalPictures(albumId); } catch (error) {
    toast(String(error));
    return {style, files: paintings};
  }
  // Keep the commissioned series in order, spreading the random folder picks across it.
  const files = [];
  let used = 0;
  for (const [index, painting] of paintings.entries()) {
    files.push(painting);
    const end = Math.round((index + 1) * personal.length / paintings.length);
    files.push(...personal.slice(used, end));
    used = end;
  }
  return {style, files};
}
function personalPictures(albumId) {
  const matching = $('imageMatch').checked && state.playing?.album.id === albumId;
  const allowAnalysis = matching && $('imageOnline').checked;
  const key = JSON.stringify([albumId, matching, allowAnalysis]);
  if (!personalSets.has(key)) {
    const request = invoke('album_images', {albumId, matching, allowAnalysis}).then(result => {
      if (result.warning) toast('Album matching unavailable · ' + result.warning, 5000);
      return result.files;
    }).catch(error => {
      if (personalSets.get(key) === request) personalSets.delete(key);
      throw error;
    });
    personalSets.set(key, request);
  }
  return personalSets.get(key);
}
function syncTv(refresh = false) {
  if (!tvActive || !tvReady) return;
  const p = state.playing;
  if (!p || p.ended || !p.playbackId) {
    librarySaver.setPicture('');
    tvChannel.postMessage({track: null});
    return;
  }
  const style = albumStyle(p.album.id);
  if (refresh || tvSet?.id !== p.album.id || tvSet.style !== style) {
    const entry = tvSet = {id: p.album.id, style, files: tvSet?.id === p.album.id ? tvSet.files : []};
    savedPaintingSet(entry.id, style).then(set => {
      if (tvSet === entry) { entry.files = set.files.map(asset); syncTv(); }
    }).catch(e => toast(String(e)));
  }
  const track = p.tracks[p.index];
  tvChannel.postMessage({track: p.playbackId, artist: track.artist || p.album.artist, title: track.title,
    intro: !p.introDone, paused: p.paused, files: tvSet.files, cover: p.album.art ? coverUrl(p.album, 'art') : '',
    spectrum: spectrum.settings()});
}
tvChannel.onmessage = e => {
  if (tvActive && e.data?.type === 'picture' && e.data.track === state.playing?.playbackId && !state.playing?.ended) {
    librarySaver.setPicture(e.data.url);
  }
  if (e.data === 'ready') { tvReady = true; syncTv(); }
  if (e.data === 'close' && tvActive) invoke('set_presenter', {display:null}).catch(e => toast(String(e)));
};
listen('presenter-closed', () => {
  tvActive = false;
  tvReady = false;
  tvSet = null;
  librarySaver.sync();
  librarySaver.setPicture('');
  show($('tvStop'), false);
});
async function openTvDialog() {
  try {
    const api = window.__TAURI__.window;
    const [screens, current] = await Promise.all([api.availableMonitors(), api.currentMonitor()]);
    const select = $('tvDisplay');
    select.replaceChildren(...screens.map((screen, i) => {
      const number = screen.name?.match(/DISPLAY(\d+)$/i)?.[1];
      const label = number ? 'Display ' + number : screen.name || 'Screen ' + (i + 1);
      return new Option(label + ' · ' + screen.size.width + ' × ' + screen.size.height,
        JSON.stringify([screen.position.x, screen.position.y]));
    }));
    const other = screens.findIndex(s => s.position.x !== current?.position.x || s.position.y !== current?.position.y);
    const saved = localStorage.getItem('muso.tvDisplay');
    if ([...select.options].some(o => o.value === saved)) select.value = saved;
    else select.selectedIndex = Math.max(0, other);
    $('tvStart').disabled = !screens.length;
    show($('tvStop'), tvActive);
    $('tvDialog').showModal();
  } catch (e) { toast(String(e)); }
}
$('tvCancel').onclick = () => $('tvDialog').close();
$('tvStop').onclick = async () => {
  try { await invoke('set_presenter', {display:null}); $('tvDialog').close(); } catch (e) { toast(String(e)); }
};
$('tvStart').onclick = async () => {
  const value = $('tvDisplay').value;
  $('tvStart').disabled = true;
  try {
    await invoke('set_presenter', {display:JSON.parse(value)});
    tvActive = true;
    localStorage.setItem('muso.tvDisplay', value);
    $('tvDialog').close();
    if (state.view === 'album') closeAlbum();
    librarySaver.sync();
    syncTv();
  } catch (e) { toast(String(e)); }
  finally { $('tvStart').disabled = false; }
};

/* ---------------- playback ---------------- */

let playbackId = 0;
const albumQueue = [];
const vinylSounds = $('vinylSounds'), runoutMinutes = $('runoutMinutes');
const runoutLess = $('runoutLess'), runoutMore = $('runoutMore');
vinylSounds.checked = localStorage.getItem('muso.vinylSounds') === '1';
function durationSetting(value) {
  const seconds = Number(value);
  return Number.isInteger(seconds) && seconds >= 1 && seconds <= 86400 ? seconds : 120;
}
let runoutDuration = durationSetting(localStorage.getItem('muso.runoutSeconds'));
const minuteFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2, useGrouping: false });
function showRunoutDuration() {
  runoutMinutes.value = minuteFormat.format(runoutDuration / 60);
  runoutMinutes.disabled = !vinylSounds.checked;
  runoutLess.disabled = !vinylSounds.checked || runoutDuration === 1;
  runoutMore.disabled = !vinylSounds.checked || runoutDuration === 86400;
}
showRunoutDuration();
function vinylTail(p) {
  const queued = albumQueue.length > 0 || p.remaining?.length > 0;
  return {runoutSeconds: vinylSounds.checked && !queued ? runoutDuration : 0,
    changeAtEnd: vinylSounds.checked && queued && p.index === p.tracks.length - 1};
}
function refreshGroove() {
  const p = state.playing;
  if (p?.playbackId && !p.ended && p.index === p.tracks.length - 1) {
    const { runoutSeconds: seconds, changeAtEnd } = vinylTail(p);
    invoke('set_runout', { playbackId: p.playbackId, seconds, changeAtEnd }).catch(e => toast(String(e)));
  }
}
vinylSounds.onchange = () => {
  localStorage.setItem('muso.vinylSounds', vinylSounds.checked ? '1' : '0');
  showRunoutDuration();
  refreshGroove();
};
function setRunoutDuration(seconds) {
  runoutDuration = clamp(seconds, 1, 86400);
  localStorage.setItem('muso.runoutSeconds', String(runoutDuration));
  showRunoutDuration();
  refreshGroove();
}
runoutLess.onclick = () => setRunoutDuration(runoutDuration - 30);
runoutMore.onclick = () => setRunoutDuration(runoutDuration + 30);
runoutMinutes.onfocus = () => runoutMinutes.select();
runoutMinutes.onchange = () => {
  const text = runoutMinutes.value.trim().replace(',', '.');
  if (!/^(?:\d+(?:\.\d*)?|\.\d+)$/.test(text)) { showRunoutDuration(); return; }
  setRunoutDuration(Math.round(Number(text) * 60));
};
runoutMinutes.onkeydown = e => {
  if (e.key === 'Enter') { e.preventDefault(); runoutMinutes.blur(); }
};
async function playIndex(i, lead = null, afterPlaybackId = null) {
  const p = state.playing;
  if (!p) return;
  if (i >= p.tracks.length) {
    const endingId = p.playbackId;
    while (manualSwitch) {
      await manualSwitch;
      if (state.playing !== p || p.playbackId !== endingId) return;
    }
    if (p.advancing) return;
    if (albumQueue.length || p.remaining?.length) {
      p.advancing = true;
      try {
        while (state.playing === p && p.playbackId === endingId && (albumQueue.length || p.remaining.length)) {
          const entry = albumQueue[0];
          const album = entry?.album ?? p.remaining.shift();
          await startAlbum(album, entry ? entry.remaining : p.remaining, true, entry);
          // A manual switch may have superseded the automatic track lookup.
          while (manualSwitch) await manualSwitch;
          if (entry && albumQueue.includes(entry)) return;
        }
      }
      finally { p.advancing = false; }
      if (state.playing !== p || p.playbackId !== endingId) return;
    }
    // The record has ended: lift the needle.
    await invoke('stop');
    if (state.playing !== p || p.playbackId !== endingId) return;
    Object.assign(p, { index: 0, paused: true, ended: true, inGroove: false, introDone: false });
    updatePlayUi();
    return;
  }
  if (p.advancing) ++albumRequest;
  Object.assign(p, { index: Math.max(0, i), paused: false, ended: false, inGroove: false, introDone: false, ending: false, playbackId: ++playbackId });
  updateAtmosView();
  await invoke('play_track', { path: p.tracks[p.index].path, duration: p.tracks[p.index].duration, playbackId: p.playbackId,
    vinylLead: vinylSounds.checked ? lead : null, afterPlaybackId, lastTrack: p.index === p.tracks.length - 1, ...vinylTail(p) });
  updatePlayUi();
}
async function togglePlay() {
  const p = state.playing;
  if (!p) return;
  if (p.ended) return playIndex(0, 'drop');
  p.paused = !p.paused;
  updatePlayUi();
  await invoke(p.paused ? 'pause' : 'resume');
}
const next = () => state.playing && playIndex(state.playing.index + 1);
const prev = () => state.playing && playIndex(state.playing.index - 1);

listen('track-ending', ({ payload }) => {
  const p = state.playing;
  if (!p || p.ended || p.playbackId !== payload) return;
  p.ending = true;
  if (state.view === 'album' && state.viewAlbum?.id === p.album.id) {
    poke();
    updateAtmosView();
  }
});
listen('track-ended', ({ payload }) => {
  if (state.playing?.playbackId === payload && !state.playing.ended) next();
});
listen('runout-started', ({ payload }) => {
  const p = state.playing;
  if (!p || p.playbackId !== payload || p.ended) return;
  p.inGroove = true;
  updatePlayUi();
});
listen('record-changing', ({ payload }) => {
  const p = state.playing;
  if (!p || p.playbackId !== payload || p.ended) return;
  p.changeId = payload;
  next();
});
listen('track-started', ({ payload }) => {
  const p = state.playing;
  if (!p || p.playbackId !== payload || p.autoGenerationChecked || !generateOnPlay.checked || !hasOpenAIKey) return;
  p.autoGenerationChecked = true;
  generate(p.album, true);
});
listen('cover-intro-ended', ({ payload }) => {
  const p = state.playing;
  if (!p || p.ended || p.playbackId !== payload) return;
  p.introDone = true;
  updateAtmosView();
});
listen('playback-error', ({ payload }) => {
  const [id, message] = payload;
  if (id === null) { toast(message, 3500); return; }
  if (state.playing?.playbackId !== id || state.playing.ended) return;
  toast(`Skipped · ${message}`, 3500);
  next();
});

const trackRail = $('alTrack');
let renderedTracks;
let selectedTrack = -1;
function centerTrack(smooth = false) {
  const button = trackRail.children[selectedTrack];
  if (!button) return;
  trackRail.scrollTo({
    left: button.offsetLeft - (trackRail.clientWidth - button.offsetWidth) / 2,
    behavior: smooth && !reducedMotion.matches ? 'smooth' : 'instant',
  });
}
function updateTrackRail(p) {
  const changedAlbum = renderedTracks !== p.tracks;
  if (changedAlbum) {
    renderedTracks = p.tracks;
    selectedTrack = -1;
    const buttons = p.tracks.map((track, index) => {
      const button = document.createElement('button');
      const number = document.createElement('span');
      number.className = 'n';
      number.textContent = String(track.trackNo ?? index + 1).padStart(2, '0');
      const showCredit = track.artist && (p.album.members?.length > 1 || track.artist !== p.album.artist);
      const label = showCredit ? track.artist + ' — ' + track.title : track.title;
      button.append(number, document.createTextNode(label));
      button.addEventListener('click', () => playIndex(index));
      return button;
    });
    trackRail.replaceChildren(...buttons);
  }
  if (selectedTrack === p.index) return;
  trackRail.children[selectedTrack]?.removeAttribute('aria-current');
  selectedTrack = p.index;
  trackRail.children[selectedTrack]?.setAttribute('aria-current', 'true');
  centerTrack(!changedAlbum);
}
let railDrag = null;
let suppressTrackClick = false;
trackRail.addEventListener('pointerdown', (e) => {
  suppressTrackClick = false;
  if (e.pointerType === 'mouse' && e.button === 0) railDrag = { x: e.clientX, left: trackRail.scrollLeft, id: e.pointerId };
  poke();
});
trackRail.addEventListener('pointermove', (e) => {
  if (!railDrag || railDrag.id !== e.pointerId || !(e.buttons & 1)) return;
  const distance = e.clientX - railDrag.x;
  if (!suppressTrackClick && Math.abs(distance) < 5) return;
  suppressTrackClick = true;
  trackRail.setPointerCapture(e.pointerId);
  trackRail.scrollLeft = railDrag.left - distance;
});
for (const event of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  trackRail.addEventListener(event, () => { railDrag = null; });
}
trackRail.addEventListener('click', (e) => {
  if (suppressTrackClick && e.detail) { e.stopImmediatePropagation(); suppressTrackClick = false; }
}, true);
trackRail.addEventListener('wheel', (e) => {
  poke();
  if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
  e.preventDefault();
  trackRail.scrollLeft += e.deltaY * (e.deltaMode === 1 ? 16 : 1);
}, { passive: false });
trackRail.addEventListener('scroll', poke, { passive: true });
trackRail.addEventListener('keydown', (e) => {
  if (e.key === ' ' || e.key === 'Enter') e.stopPropagation();
});
new ResizeObserver(() => centerTrack()).observe(trackRail);

const miniMeter = createMiniMeter(invoke, listen, $('npBars'));
let npAlbum = null;
function syncMini() {
  const p = state.playing;
  miniMeter.set(!!p && !p.paused && !p.ended && state.view === 'library', p?.playbackId);
}
function updatePlayUi() {
  show($('runoutLabel'), !!state.playing?.inGroove);
  updateAtmosView();
  const p = state.playing;
  const np = $('nowPlaying');
  show(np, !!p);
  syncMini();
  if (!p) return;
  np.classList.toggle('paused', p.paused || p.ended);
  if (npAlbum !== p.album.id) {
    npAlbum = p.album.id;
    const img = $('npCover');
    img.parentElement.style.setProperty('--blank', blankColor(p.album.id));
    if (p.album.art) { img.src = coverUrl(p.album, 'thumb'); show(img); }
    else { img.removeAttribute('src'); show(img, false); }
    $('npArtist').textContent = p.album.artist;
    $('npTitle').textContent = p.album.title;
    np.classList.remove('swap');
    void np.offsetWidth;
    np.classList.add('swap');
  }
  if (state.viewAlbum && state.viewAlbum.id === p.album.id) {
    $('album').classList.toggle('is-paused', p.paused);
    updateTrackRail(p);
  }
}

$('playBtn').addEventListener('click', togglePlay);
$('nextBtn').addEventListener('click', next);
$('prevBtn').addEventListener('click', prev);
$('nowPlaying').addEventListener('click', () => state.playing && displayAlbum(state.playing.album));

/* ---------------- album view ---------------- */

let albumRequest = 0;
let inspectingCover = false;
const coverStack = document.querySelector('.cover-stack');

function layoutQueue() {
  coverStack.style.setProperty('--queue-size', albumQueue.length);
  coverStack.classList.toggle('fanned', albumQueue.length >= 2);
  albumQueue.forEach(({ album, button }, i) => {
    button.style.setProperty('--slot', i);
    button.style.zIndex = 5 - i;
    button.setAttribute('aria-label', `Queue position ${i + 1}. ${album.artist} — ${album.title}. Swipe up to play or down to remove.`);
    button.querySelector('.swipe-hint').textContent = albumQueue.length === 1 ? '↑ Play · ↓ Remove' : `${i + 1} ↕`;
  });
}

function removeQueued(entry) {
  const i = albumQueue.indexOf(entry);
  if (i < 0) return;
  albumQueue.splice(i, 1);
  entry.button.remove();
  layoutQueue();
}

function enqueue(album, remaining = []) {
  const button = $('queueSleeve').content.firstElementChild.cloneNode(true);
  const entry = { album, button, remaining };
  const img = button.querySelector('img');
  const label = button.querySelector('.queue-title');
  label.textContent = album.title;
  if (album.art) img.src = coverUrl(album, 'art');
  show(img, !!album.art);
  show(label, !album.art);
  img.onerror = () => { show(img, false); show(label); };
  button.style.setProperty('--blank', blankColor(album.id));
  let drag;
  const playNow = () => startAlbum(album, entry.remaining, false, entry);
  const dismiss = () => {
    const position = albumQueue.indexOf(entry);
    removeQueued(entry);
    refreshGroove();
    updateAtmosView();
    (albumQueue[position]?.button ?? albumQueue.at(-1)?.button ?? $('playBtn')).focus({ preventScroll: true });
    toast(`Removed from queue · ${album.title}`);
  };
  button.onpointerdown = e => {
    if (!e.isPrimary || e.button !== 0 || drag) return;
    drag = { id: e.pointerId, x: e.clientX, y: e.clientY, threshold: clamp(button.offsetHeight * .14, 36, 80) };
    button.setPointerCapture(e.pointerId);
    button.classList.add('dragging');
    poke();
  };
  button.onpointermove = e => {
    if (drag?.id !== e.pointerId) return;
    const distance = e.clientY - drag.y;
    button.style.setProperty('--lift', distance + 'px');
    const progress = clamp(distance / drag.threshold, 0, 1);
    button.style.opacity = String(1 - progress * progress * (3 - 2 * progress));
  };
  function resetDrag() {
    button.classList.remove('dragging');
    button.style.removeProperty('--lift');
    button.style.removeProperty('opacity');
  }
  const release = e => {
    if (drag?.id !== e.pointerId) return;
    const dy = drag.y - e.clientY, dx = e.clientX - drag.x, threshold = drag.threshold;
    drag = null;
    if (e.type !== 'pointerup' || Math.abs(dy) < threshold || Math.abs(dy) <= Math.abs(dx)) {
      resetDrag();
      poke();
      return;
    }
    if (dy > 0) {
      button.style.opacity = '0';
      playNow().finally(resetDrag);
    }
    else dismiss();
  };
  button.onpointerup = button.onpointercancel = button.onlostpointercapture = release;
  button.onkeydown = e => {
    if (['Enter', ' '].includes(e.key)) e.stopPropagation();
    if (e.key === 'ArrowUp') { e.preventDefault(); e.stopPropagation(); playNow(); }
    if (e.key === 'ArrowDown') { e.preventDefault(); e.stopPropagation(); dismiss(); }
  };
  button.onclick = e => { if (e.detail === 0) playNow(); };
  button.onfocus = poke;
  coverStack.insertBefore(button, $('cover'));
  albumQueue.push(entry);
  return entry;
}

function openLibraryAlbum(album) {
  if (!activeCrate) return openAlbum(album);
  const index = activeCrate.albums.findIndex(entry => entry.id === album.id);
  if (index < 0) return openAlbum(album);
  const remaining = activeCrate.albums.slice(index + 1).map(entry => albumsById.get(entry.id)).filter(Boolean);
  return openAlbum(album, remaining);
}

function openAlbum(a, remaining) {
  const p = state.playing;
  if (p?.album.id === a.id) {
    if (remaining === undefined) return displayAlbum(a);
    if (p.ended || p.inGroove || p.advancing) return startAlbum(a, remaining);
    p.remaining = remaining;
    refreshGroove();
    if (p.paused) togglePlay();
    return displayAlbum(a);
  }
  let entry = albumQueue.find(item => item.album.id === a.id);
  if (!p || p.ended || p.inGroove || p.paused)
    return startAlbum(a, remaining, false, entry);

  if (entry) entry.remaining = remaining ?? [];
  else if (albumQueue.length < 5) entry = enqueue(a, remaining);
  layoutQueue();
  refreshGroove();
  inspectingCover = true;
  displayAlbum(p.album);
  if (entry) entry.button.focus({ preventScroll: true });
  else toast('Queue full · 5 records are waiting');
}

const playerCover = $('cover');
let coverPull = null;
function resetCoverPull() {
  const pull = coverPull;
  coverPull = null;
  if (pull && playerCover.hasPointerCapture(pull.id)) playerCover.releasePointerCapture(pull.id);
  albumCrates.dismissPlayerTray();
}
playerCover.addEventListener('pointerdown', e => {
  if (!e.isPrimary || e.button !== 0 || coverPull || !state.viewAlbum) return;
  coverPull = { id: e.pointerId, x: e.clientX, y: e.clientY, album: state.viewAlbum, filing: false };
  playerCover.setPointerCapture(e.pointerId);
  poke();
});
playerCover.addEventListener('pointermove', e => {
  if (coverPull?.id !== e.pointerId) return;
  if (!coverPull.filing) {
    if (Math.hypot(e.clientX - coverPull.x, e.clientY - coverPull.y) < 8) return;
    coverPull.filing = true;
    albumCrates.begin(coverPull.album, playerCover, coverPull.x, coverPull.y, true);
  }
  albumCrates.drag(e.clientX, e.clientY);
});
for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  playerCover.addEventListener(type, e => {
    if (coverPull?.id !== e.pointerId) return;
    const filing = coverPull.filing;
    coverPull = null;
    if (filing) albumCrates.end(e.clientX, e.clientY, type !== 'pointerup');
    updateAtmosView();
    poke();
  });
}
function clearCrateTransition() {
  const view = $('album');
  view.classList.remove('changing-record', 'promoting-record', 'waiting-for-cover');
  view.querySelectorAll('.outgoing-sleeve, .outgoing-backdrop').forEach(layer => layer.remove());
}

function animateCrateChange(nextImage = new Image()) {
  const view = $('album');
  const sleeve = $('cover').cloneNode(false);
  sleeve.removeAttribute('id');
  // Keep the painted image in the outgoing sleeve, including while the new art decodes.
  const previousImage = $('coverImg');
  nextImage.id = 'coverImg';
  nextImage.alt = '';
  previousImage.replaceWith(nextImage);
  previousImage.removeAttribute('id');
  sleeve.append(previousImage);
  sleeve.classList.add('outgoing-sleeve');
  sleeve.style.setProperty('--blank', view.style.getPropertyValue('--blank'));
  const outgoingOpacity = Number(getComputedStyle(coverStack).opacity) > 0 ? getComputedStyle($('cover')).opacity : '0';
  view.style.setProperty('--outgoing-opacity', outgoingOpacity);
  sleeve.setAttribute('aria-hidden', 'true');
  $('cover').after(sleeve);

  const backdrop = document.createElement('div');
  backdrop.className = 'outgoing-backdrop';
  backdrop.setAttribute('aria-hidden', 'true');
  // Preserve the painting currently on screen, including a crossfade in progress.
  const atmosphere = view.querySelector('.atmos');
  const atmosphereOpacity = Number(getComputedStyle(atmosphere).opacity);
  for (const source of [$('bgArt'), ...atmosphere.querySelectorAll('img')]) {
    if (!source.currentSrc || !source.naturalWidth || source.classList.contains('hidden')) continue;
    const style = getComputedStyle(source.closest('.painting-layer') || source);
    const opacity = source === $('bgArt') ? 1 : Number(style.opacity) * atmosphereOpacity;
    if (!opacity) continue;
    const image = new Image();
    image.src = source.currentSrc;
    Object.assign(image.style, { inset: style.inset, width: style.width, height: style.height,
      opacity, transform: style.transform, filter: style.filter });
    backdrop.append(image);
  }
  atmosphere.after(backdrop);
  view.classList.add('changing-record');
  backdrop.addEventListener('animationend', () => { if (backdrop.isConnected) clearCrateTransition(); }, { once: true });
  return backdrop;
}

let manualSwitch;
function startAlbum(a, remaining = [], advancing = false, entry) {
  if (advancing) return switchAlbum(a, remaining, true, entry);
  const request = switchAlbum(a, remaining, false, entry);
  manualSwitch = request;
  return request.finally(() => { if (manualSwitch === request) manualSwitch = null; });
}

async function switchAlbum(a, remaining, advancing, entry) {
  const request = ++albumRequest;
  const controller = tvActive && state.view === 'album' && !advancing;
  const previousView = state.view;
  try {
    const tracks = await invoke('get_tracks', {albumId: a.id});
    if (request !== albumRequest || (entry && !albumQueue.includes(entry))) return;
    if (!tracks.length) throw new Error('No tracks available for this album.');
    const preview = entry && state.view === 'album' && !advancing ? entry.button.querySelector('img') : null;
    const previousLeft = preview ? coverStack.getBoundingClientRect().left : 0;
    if (preview) preview.onerror = null;
    if (entry) removeQueued(entry);
    personalSets.clear();
    const outgoing = state.playing;
    const lead = outgoing && !outgoing.ended && outgoing.album.id !== a.id ? 'change' : 'drop';
    const after = advancing && outgoing?.changeId === outgoing?.playbackId ? outgoing?.changeId : null;
    state.playing = { album: a, tracks, index: 0, paused: false, ended: false, remaining };
    if (state.view === 'album' || (!advancing && !tvActive && previousView === state.view)) {
      displayAlbum(a, {animate: advancing || (!!preview && !controller), preview, previousLeft});
    }
    await playIndex(0, lead, after);
    if (controller && request === albumRequest && state.view === 'album') closeAlbum();
  } catch (e) {
    if (request !== albumRequest) return;
    if (entry) { removeQueued(entry); refreshGroove(); }
    updatePlayUi();
    toast(String(e), 4500);
  }
}

function displayAlbum(a, {animate = false, preview, previousLeft} = {}) {
  resetCoverPull();
  clearCrateTransition();
  const outgoing = animate && state.viewAlbum?.id !== a.id && !reducedMotion.matches && !document.hidden && animateCrateChange(preview || undefined);
  if (preview && outgoing) {
    $('album').classList.add('promoting-record');
    const shift = previousLeft - coverStack.getBoundingClientRect().left;
    coverStack.querySelector('.outgoing-sleeve').style.translate = shift + 'px';
    if (shift) {
      for (const {button} of albumQueue) {
        button.animate([{translate:shift + 'px'}, {translate:'0px'}], {duration:480, easing:'cubic-bezier(.22,.61,.36,1)'});
      }
    }
  } else if (preview) {
    $('coverImg').replaceWith(preview);
    preview.id = 'coverImg';
  }
  closeSearch();
  closeMenu();
  if (state.view !== 'album') playerVolume.refresh();
  state.view = 'album';
  state.viewAlbum = a;
  renderCoverSearchStatus();
  const view = $('album');
  view.classList.remove('leaving', 'idle');
  view.style.setProperty('--blank', blankColor(a.id));
  const cover = $('coverImg');
  const bg = $('bgArt');
  if (a.art) {
    const url = coverUrl(a, 'art');
    if (cover.src !== url) cover.src = url;
    if (bg.src !== url) bg.src = url;
    show(cover); show(bg);
    if (outgoing) {
      view.classList.add('waiting-for-cover');
      cover.decode().catch(() => {
        if (outgoing.isConnected && cover.src === url && !cover.naturalWidth) show(cover, false);
      }).finally(() => { if (outgoing.isConnected) view.classList.remove('waiting-for-cover'); });
    }
  } else {
    show(cover, false); show(bg, false);
  }
  $('alArtist').textContent = a.artist;
  $('alTitle').textContent = a.title;
  showStyle(a.id);
  show($('albumMenu'), false);
  $('atmosStatus').textContent = '';
  show(view);
  show($('menuBtn'), false);

  updatePlayUi();
  startAtmospheres(a.id);
  poke();
}

function closeAlbum() {
  playerVolume.close();
  resetCoverPull();
  inspectingCover = false;
  const view = $('album');
  view.classList.add('leaving');
  state.view = 'library';
  state.viewAlbum = null;
  show($('menuBtn'));
  syncMini();
  const finish = () => {
    show(view, false);
    view.classList.remove('leaving');
    clearCrateTransition();
    stopAtmospheres();
  };
  const exit = view.getAnimations().find(animation => animation.transitionProperty === '--motion-opacity');
  if (exit) exit.finished.then(finish, () => {});
  else finish();
}
$('backBtn').addEventListener('click', closeAlbum);

// Calm mode: chrome fades away when the mouse rests.
let idleTimer;
function poke() {
  if ($('album').classList.contains('idle')) centerTrack();
  $('album').classList.remove('idle');
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (state.view === 'album' && !coverPull && !coverStack.querySelector('.dragging') && !$('playerVolume').classList.contains('open') && $('albumMenu').classList.contains('hidden')) {
      $('album').classList.add('idle');
      inspectingCover = false;
      updateAtmosView();
    }
  }, 3500);
}
for (const event of ['pointerdown', 'pointermove']) $('album').addEventListener(event, poke, {passive:true});
$('album').addEventListener('click', (e) => {
  if ($('album').classList.contains('showing-atmos') && !e.target.closest('button, .popover, .controls, .al-track, #playerAudio')) {
    inspectingCover = true;
    updateAtmosView();
  }
  poke();
  if (!e.target.closest('#albumMenu, #albumMenuBtn')) show($('albumMenu'), false);
});
const playerWindow = window.__TAURI__.window.getCurrentWindow();
const exitFullscreen = $('exitFullscreen');
let windowFullscreen = false;
function showFullscreenAction(fullscreen) {
  windowFullscreen = fullscreen;
  if (!fullscreen) show(exitFullscreen, false);
  document.querySelectorAll('.fullscreen-label').forEach(el => { el.textContent = fullscreen ? 'Exit fullscreen' : 'Enter fullscreen'; });
  document.querySelectorAll('.fullscreen-icon').forEach(el => el.setAttribute('href', 'icons/lucide.svg#' + (fullscreen ? 'minimize' : 'maximize')));
}
async function toggleFullscreen(target) {
  try {
    const enter = target ?? !(await playerWindow.isFullscreen());
    await playerWindow.setFullscreen(enter);
    showFullscreenAction(enter);
  } catch (e) { toast(String(e)); }
}
function syncWindowFullscreen() {
  playerWindow.isFullscreen().then(showFullscreenAction).catch(console.warn);
}
syncWindowFullscreen();
let fullscreenResizeTimer;
window.addEventListener('resize', () => {
  clearTimeout(fullscreenResizeTimer);
  fullscreenResizeTimer = setTimeout(syncWindowFullscreen, 120);
});
document.addEventListener('pointermove', e => {
  if (!windowFullscreen) return;
  const inReach = (e.clientY <= 10 && !e.buttons) || (!exitFullscreen.classList.contains('hidden') && e.clientY <= 64);
  show(exitFullscreen, inReach && e.pointerType === 'mouse');
}, { passive: true });
document.addEventListener('pointerdown', e => {
  if (e.pointerType !== 'mouse') show(exitFullscreen, false);
}, { capture: true, passive: true });
document.documentElement.addEventListener('pointerleave', () => show(exitFullscreen, false));
window.addEventListener('blur', () => show(exitFullscreen, false));
exitFullscreen.addEventListener('click', () => toggleFullscreen(false));
$('albumMenuBtn').addEventListener('click', async () => {
  const opening = $('albumMenu').classList.contains('hidden');
  if (opening && state.viewAlbum) fillAlbumGenre(state.viewAlbum);
  show($('albumMenu'), opening);
  showFullscreenAction(await playerWindow.isFullscreen());
});
$('fullscreenBtn').addEventListener('click', () => { show($('albumMenu'), false); toggleFullscreen(); });
function openEQ(fromPlayer) {
  if (fromPlayer) show($('albumMenu'), false);
  $('eq').classList.toggle('from-player', fromPlayer);
  show($('eq'));
  $('spectrumToggle').focus({ preventScroll: true });
}
$('albumEQ').addEventListener('click', () => openEQ(true));
$('albumHeos').addEventListener('click', async () => {
  show($('albumMenu'), false);
  await playerVolume.settled();
  heosOutput.openVolume();
});
$('heosVolumeDialog').addEventListener('close', () => { playerVolume.refresh(); poke(); $('albumMenuBtn').focus(); });
$('eq').querySelector('.close').addEventListener('click', () => {
  if (!$('eq').classList.contains('from-player')) { openMenu(); $('menu').querySelector('[data-action="eq"]').focus({ preventScroll: true }); }
  else { poke(); $('albumMenuBtn').focus(); }
});
$('openImageFolder').addEventListener('click', async () => {
  show($('albumMenu'), false);
  if (!state.viewAlbum) return;
  try { await invoke('open_image_folder', { albumId: state.viewAlbum.id }); }
  catch (e) { toast(String(e), 5000); }
});
const deleteAtmosDialog = $('deleteAtmosDialog');
$('deleteAtmosBtn').addEventListener('click', () => {
  if (!visiblePainting || visiblePainting.set.albumId !== state.viewAlbum?.id) return;
  const {set, file} = visiblePainting, album = state.viewAlbum;
  show($('albumMenu'), false);
  $('deleteAtmosPreview').src = asset(file);
  $('deleteAtmosAlbum').textContent = [album.artist, album.title].filter(Boolean).join(' — ');
  deleteAtmosDialog.returnValue = '';
  deleteAtmosDialog.addEventListener('close', async () => {
    $('deleteAtmosPreview').removeAttribute('src');
    if (state.view === 'album') $('albumMenuBtn').focus({preventScroll:true});
    if (deleteAtmosDialog.returnValue !== 'delete') return;
    try {
      await invoke('delete_atmosphere', {albumId:album.id, style:set.style, file});
      if (state.playing?.album.id === album.id) syncTv(true);
      if (state.viewAlbum?.id === album.id) {
        const next = playlist?.shown === file
          ? playlist.files[(playlist.files.indexOf(file) + 1) % playlist.files.length] : playlist?.shown;
        await startAtmospheres(album.id, false, next);
      }
      toast('Atmosphere deleted');
    } catch (e) { toast(String(e), 5000); }
  }, {once:true});
  deleteAtmosDialog.showModal();
});
$('showAtmosBtn').addEventListener('click', () => {
  show($('albumMenu'), false);
  setAtmosEnabled($('showAtmosBtn').getAttribute('aria-pressed') !== 'true');
});
$('genBtn').addEventListener('click', () => {
  show($('albumMenu'), false);
  if (state.viewAlbum) generate(state.viewAlbum);
});

/* ---------------- atmospheres ---------------- */

// Keep the repeating texture inside its own bitmap. WebKitGTK can tile the
// underlying painting when a repeating CSS background blends across a fade.
function initAtmosGrain() {
  const canvas = $('atmosGrain');
  const texture = new Image();
  texture.onload = () => {
    const ctx = canvas.getContext('2d');
    const pattern = ctx.createPattern(texture, 'repeat');
    function paint() {
      const width = Math.round(canvas.clientWidth * devicePixelRatio);
      const height = Math.round(canvas.clientHeight * devicePixelRatio);
      if (canvas.width === width && canvas.height === height) return;
      canvas.width = width;
      canvas.height = height;
      ctx.fillStyle = pattern;
      ctx.fillRect(0, 0, width, height);
    }
    const observer = new ResizeObserver(paint);
    const box = 'devicePixelContentBoxSize' in ResizeObserverEntry.prototype ? 'device-pixel-content-box' : 'content-box';
    observer.observe(canvas, { box });
    function watchDensity() {
      paint();
      matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener('change', watchDensity, { once: true });
    }
    watchDensity();
  };
  texture.src = 'grain.png';
}
initAtmosGrain();

let atmosTimer = null;
let atmosToken = 0;
let readyAlbum = null;
let playlist = null;
let visiblePainting = null;
function updateAtmosView() {
  syncTv();
  const p = state.playing;
  const reveal = !coverPull && !inspectingCover && p?.introDone && !p.ending && readyAlbum === p.album.id && state.viewAlbum?.id === readyAlbum;
  $('album').classList.toggle('showing-atmos', !!reveal);
  if (reveal) {
    visiblePainting = playlist?.shown?.startsWith('atmos/') ? {set:playlist, file:playlist.shown} : null;
  } else visiblePainting = null;
  show($('deleteAtmosBtn'), !!visiblePainting);
  coverStack.inert = !!reveal;
  spectrum.setPlayback(!!reveal && !p.paused, p?.playbackId);
  if (!reveal) stopWave();
}
async function startAtmospheres(albumId, announce = false, preferredFile) {
  stopAtmospheres();
  const token = ++atmosToken;
  const style = albumStyle(albumId);
  try {
    // Preview the chosen look on a saved set until this album has images in that style.
    const set = await savedPaintingSet(albumId, style);
    if (token !== atmosToken) return;
    if (set.files.length) {
      if (atmosEnabled()) setPaintings(albumId, set.style, set.files, preferredFile);
      else $('showAtmosBtn').disabled = false;
      return;
    }
  }
  catch (e) { if (token === atmosToken) toast(String(e), 6000); return; }
  if (announce) toast('No images available. Add a folder in Images or generate atmospheres.', 5500);
}

function schedulePainting() {
  clearTimeout(atmosTimer);
  if (!playlist || !playlist.shown || playlist.pending || playlist.files.length < 2) return;
  const delay = Math.max(0, playlist.since + ATMOS_INTERVAL - performance.now());
  atmosTimer = setTimeout(() => {
    const next = (playlist.files.indexOf(playlist.shown) + 1) % playlist.files.length;
    loadPainting(playlist, playlist.files[next]);
  }, delay);
}

function setPaintings(albumId, style, files, preferredFile) {
  if (!files.length) return;
  ++atmosToken; // Discard any library lookup that began before this set arrived.
  if (playlist?.albumId === albumId && playlist.style === style && playlist.first === files[0]) {
    playlist.files = files.filter(file => !playlist.failed.has(file));
    schedulePainting();
    return;
  }
  clearTimeout(atmosTimer);
  playlist = { albumId, style, first: files[0], files, failed: new Set() };
  loadPainting(playlist, files.includes(preferredFile) ? preferredFile : files[0]);
}

async function loadPainting(set, file) {
  set.pending = true;
  const other = $('atmosA').classList.contains('on') ? $('atmosA') : $('atmosB');
  const img = other === $('atmosA') ? $('atmosB') : $('atmosA');
  img.src = asset(file);
  try {
    await img.decode();
  } catch {
    if (set !== playlist) return;
    set.pending = false;
    set.failed.add(file);
    set.files = set.files.filter(f => f !== file);
    if (set.shown) { schedulePainting(); return; }
    if (set.files.length) { loadPainting(set, set.files[0]); return; }
    stopAtmospheres();
    toast('Saved atmosphere images could not be opened.', 5000);
    return;
  }
  if (set !== playlist) return;
  set.pending = false;
  set.shown = file;
  set.since = performance.now();
  $('paintingCredit').hidden = true;
  invoke('painting_credit', { albumId:set.albumId, style:set.style, file }).then(credit => {
    if (playlist !== set || set.shown !== file || !credit) return;
    $('paintingTitle').textContent = credit.title;
    $('paintingByline').textContent = [credit.style, credit.persona].filter(Boolean).join(' · ');
    $('paintingCredit').hidden = false;
  }).catch(() => {});
  readyAlbum = set.albumId;
  $('album').dataset.preview = file.startsWith('personal/') || set.style === albumStyle(set.albumId) ? '' : albumStyle(set.albumId);
  img.classList.add('on');
  other.classList.remove('on');
  updateAtmosView();
  $('showAtmosBtn').disabled = false;
  $('showAtmosBtn').setAttribute('aria-pressed', 'true');
  schedulePainting();
}
function stopAtmospheres() {
  atmosToken++;
  playlist = null;
  clearTimeout(atmosTimer);
  atmosTimer = null;
  readyAlbum = null;
  $('paintingCredit').hidden = true;
  updateAtmosView();
  $('showAtmosBtn').disabled = true;
  $('showAtmosBtn').setAttribute('aria-pressed', 'false');
  for (const id of ['atmosA', 'atmosB']) {
    const img = $(id);
    img.classList.remove('on');
    img.removeAttribute('src');
  }
}

const generating = new Set();
async function paintingReady({ albumId, style, files }) {
  if (tvActive && state.playing?.album.id === albumId) syncTv(true);
  if (state.viewAlbum?.id !== albumId || !files.length) return;
  if (!atmosEnabled()) {
    $('showAtmosBtn').disabled = false;
  } else if (albumStyle(albumId) === style || !playlist || playlist.style === 'personal') {
    const token = ++atmosToken;
    const series = await albumPaintingSeries(albumId, style, files);
    if (token === atmosToken) setPaintings(albumId, style, series.files);
  }
}
listen('atmos-image-ready', ({ payload }) => {
  paintingReady(payload);
  if (payload.firstInSeries && styleAlbum === payload.albumId) showStyle(payload.albumId);
});
async function generate(a, automatic = false) {
  const style = albumStyle(a.id);
  const persona = albumPersona(a.id);
  if (!automatic && !(await invoke('has_openai_key'))) {
    await loadKey();
    prepareAi();
    show($('ai'));
    toast('Add an OpenAI API key first');
    return;
  }
  if (generating.has(a.id)) return;
  generating.add(a.id);
  if (!automatic) toast(`Composing atmospheres for ${a.title}…`);
  try {
    const files = await invoke('generate_atmospheres', { albumId: a.id, research: researchEnabled(), allowRepaint: repaintEnabled(), style,
      artistPersonas: persona === 'random', artistPersona: ['random', 'none'].includes(persona) ? null : persona,
      imageSettings: { model: $('imageModel').value, resolution: $('imageResolution').value },
      onlyIfEmpty: automatic, count: automatic ? Number(playImageCount.value) : 3 });
    if (!files.length) return;
    paintingReady({ albumId: a.id, style, files });
    toast(`New atmospheres saved · ${a.title}`);
  } catch (e) {
    toast(String(e), 6000);
  } finally {
    generating.delete(a.id);
    if (state.viewAlbum && state.viewAlbum.id === a.id) $('atmosStatus').textContent = '';
  }
}
listen('atmos-progress', ({ payload }) => {
  if (state.viewAlbum && state.viewAlbum.id === payload.albumId) {
    $('atmosStatus').textContent = `${payload.message} · ${payload.step}/${payload.total - 1}`;
  }
});

/* ---------------- keyboard ---------------- */

window.addEventListener('keydown', (e) => {
  if (['clearSearch', 'exitFullscreen'].includes(e.target.id) && (e.key === 'Enter' || e.key === ' ')) return;
  const modalOpen = [...document.querySelectorAll('.modal')].some((m) => !m.classList.contains('hidden'));
  if (document.querySelector('.library-dialog[open]')) return;
  if (e.key === 'F11') {
    e.preventDefault();
    if (!e.repeat) toggleFullscreen();
    return;
  }
  if ($('coverConfirm').open || stylePicker.open || personaPicker.open || (e.target.closest('.style-trigger') && [' ', 'Enter'].includes(e.key))) return;
  if (e.key === 'Escape' && !$('eq').classList.contains('hidden')) {
    $('eq').querySelector('.close').click();
    return;
  }
  if (e.target.matches('input, select, textarea')) return;
  if (e.key === 'Escape') {
    if (coverPull) { resetCoverPull(); updateAtmosView(); return; }
    if (modalOpen) document.querySelectorAll('.modal').forEach((m) => show(m, false));
    else if (!$('menu').classList.contains('hidden')) closeMenu();
    else if (!$('search').classList.contains('hidden')) closeSearch();
    else if (state.view === 'library' && albumCrates.dismissDock()) return;
    else if (state.view === 'crates') albumCrates.escape();
    else if (state.view === 'genres') genresView.escape();
    else if (state.view === 'album') closeAlbum();
    else if (tvActive && navigator.userAgent.includes('Linux')) invoke('set_presenter', {display:null}).catch(e => toast(String(e)));
    return;
  }
  if (modalOpen || !$('menu').classList.contains('hidden')) return;
  if (e.target.closest('#crateDock, #libraryCrateHeading, #editLibraryCrate, #fanTools, #cratesBack') && [' ', 'Enter'].includes(e.key)) return;
  if (e.target === $('noteAction') && (e.key === ' ' || e.key === 'Enter')) return;
  if (libraryIndex.contains(e.target) && (e.key === 'Enter' || e.key === ' ')) return;
  if (e.key === 'f' && state.view === 'album') {
    toggleFullscreen();
    e.preventDefault();
    return;
  }
  if (e.key === ' ') {
    if (state.view === 'genres' && e.target.closest?.('#genresPage button')) return;
    togglePlay(); e.preventDefault(); return;
  }

  if (state.view === 'album') {
    if (e.key === 'ArrowRight') next();
    else if (e.key === 'ArrowLeft') prev();
    else if (e.key === 'Backspace') closeAlbum();
    poke();
    return;
  }
  if (state.view !== 'library' || libraryGestures.active || !state.albums.length) return;
  const t = Math.round(state.target);
  if (e.key === 'ArrowRight' || e.key === 'ArrowDown') go(t + 1);
  else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') go(t - 1);
  else if (e.key === 'PageDown') go(t + 10);
  else if (e.key === 'PageUp') go(t - 10);
  else if (e.key === 'Home') go(0);
  else if (e.key === 'End') go(state.albums.length - 1);
  else if (e.key === 'Enter') openLibraryAlbum(focused());
  else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') { openSearch(); e.preventDefault(); }
  else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && /\S/.test(e.key)) {
    // Just start typing to dig.
    e.preventDefault();
    openSearch(e.key);
  } else return;
  e.preventDefault();
});

window.addEventListener('resize', layout);
window.addEventListener('contextmenu', (e) => e.preventDefault());

boot().catch((e) => toast(String(e), 8000));
