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
const libraryIndex = $('libraryIndex');
const indexLetters = $('indexLetters');
const letters = '#ABCDEFGHIJKLMNOPQRSTUVWXYZ';
const letterButtons = new Map();
const titleOrder = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
let indexMode = localStorage.getItem('muso.indexMode') === 'album' ? 'album' : 'artist';
let artistAlbums = [], titleAlbums = [];
let preferredMember;
let currentLetter;

function albumInitial(album) {
  const name = indexMode === 'album' ? album.title.trim() : album.artist.trim().replace(/^the /i, '').trimStart();
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

function indexAlbums(albums) {
  const byTitle = indexMode === 'album';
  const label = byTitle ? 'Album' : 'Artist';
  $('indexModeLabel').textContent = label;
  $('indexMode').title = byTitle ? 'Switch to artist order' : 'Switch to album title order';
  $('indexMode').setAttribute('aria-label', label + ' order. ' + $('indexMode').title);
  indexLetters.setAttribute('aria-label', label + ' initials');
  const groups = new Map([...letters].map(letter => [letter, []]));
  for (const a of albums) {
    a.initial = albumInitial(a);
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
  const index = Math.round(state.target);
  const id = state.albums[index]?.id;
  if (!id || (savedPosition?.id === id && savedPosition.index === index)) return;
  savedPosition = { id, index };
  localStorage.setItem(positionKey, JSON.stringify(savedPosition));
}
window.addEventListener('pagehide', rememberPosition);
document.addEventListener('visibilitychange', () => { if (document.hidden) rememberPosition(); });

function layout() {
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
  for (const el of pool.values()) el.remove();
  pool.clear();
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
  const S = state.size;
  const side = S * 0.66;
  const gap = S * 0.17;
  for (let i = lo; i <= hi; i++) {
    let el = pool.get(i);
    if (!el) { el = makeSleeve(i); pool.set(i, el); crate.appendChild(el); }
    const d = i - state.pos;
    const ad = Math.abs(d);
    const s = Math.sign(d);
    const t = Math.min(ad, 1);
    const x = s * (t * side + Math.max(0, ad - 1) * gap);
    const z = -t * S * 0.6 - Math.max(0, ad - 1) * S * 0.05;
    const rot = -s * t * 64;
    el.style.transform = `translate3d(${x.toFixed(1)}px,0,${z.toFixed(1)}px) rotateY(${rot.toFixed(2)}deg)`;
    el.style.zIndex = String(1000 - Math.round(ad * 10));
    el.style.opacity = ad > WINDOW - 1.5 ? String(clamp(WINDOW - 0.5 - ad, 0, 1)) : '1';
    el.style.setProperty('--shade', (Math.min(ad, 5) * 0.16).toFixed(3));
  }
}

let lastCaption = -1;
let noteTimer, noteRequest = 0, visibleNote = null;
const pendingArticles = new Map();
let articleAlbum;
function renderNote() {
  const { album, body } = visibleNote;
  const stage = pendingArticles.get(album.id);
  $('noteTeaser').textContent = body?.split(/\n\s*\n/)[0] || '';
  $('noteAction').textContent = body ? 'Read album article' : 'Write album article with AI';
  show($('noteAction'), !stage);
  show($('noteProgress'), Boolean(stage));
  $('noteStage').textContent = stage || '';
  show($('notePreview'));
}
listen('article-progress', ({ payload: [id, stage] }) => {
  if (!pendingArticles.has(id)) return;
  pendingArticles.set(id, stage);
  if (visibleNote?.album.id === id) renderNote();
});
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
  if (open) $('rewriteArticle').focus();
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
  if (!album || pendingArticles.has(album.id)) return;
  show($('article'), false);
  const deleting = action === 'delete';
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
      $('archiveStatus').textContent = result ? (importing ? 'Imported' : 'Exported') + ' · ' + result.notes + ' articles · ' + result.images + ' images' + (result.skipped ? ' · ' + result.skipped + ' unmatched albums' : '') : '';
      if (result && importing) await loadAlbums();
    } catch(e) { $('archiveStatus').textContent = String(e); }
    finally { $('exportArchive').disabled = $('importArchive').disabled = false; }
  });
}
function updateCaption(force) {
  const n = state.albums.length;
  if (!n) return;
  const i = clamp(Math.round(state.pos), 0, n - 1);
  if (i === lastCaption && !force) return;
  lastCaption = i;
  const a = state.albums[i];
  prepareNote(a);
  if (currentLetter !== a.initial) {
    const previous = letterButtons.get(currentLetter);
    if (previous) {
      previous.removeAttribute('aria-current');
      previous.tabIndex = -1;
    }
    const button = letterButtons.get(a.initial);
    button.setAttribute('aria-current', 'true');
    button.tabIndex = 0;
    currentLetter = a.initial;
  }
  $('capArtist').textContent = a.artist;
  const title = $('capTitle');
  title.textContent = a.title;
  const details = [a.year, a.genre?.trim()].filter(Boolean);
  if (details.length) {
    const meta = document.createElement('span');
    meta.className = 'album-meta';
    meta.textContent = details.join(' · ');
    title.append(' ', meta);
  }
  $('counter').textContent = `${String(i + 1).padStart(4, '0')} / ${String(n).padStart(4, '0')}`;
}

let animating = false;
function tick() {
  const diff = state.target - state.pos;
  if (Math.abs(diff) < 0.0015 && !dragging) {
    state.pos = state.target;
    animating = false;
  } else {
    state.pos += diff * (dragging ? 0.45 : 0.16);
  }
  render();
  updateCaption();
  if (animating) requestAnimationFrame(tick);
}
function kick() {
  if (!animating) { animating = true; requestAnimationFrame(tick); }
}
function go(index) {
  const n = state.albums.length;
  if (!n) return;
  const t = clamp(Math.round(index), 0, n - 1);
  // Long jumps (search) start just before the destination so we don't stream thousands of sleeves.
  if (Math.abs(t - state.pos) > 24) state.pos = t - Math.sign(t - state.pos) * 6;
  state.target = t;
  rememberPosition();
  kick();
}
const focused = () => state.albums[clamp(Math.round(state.target), 0, state.albums.length - 1)];

// wheel / trackpad
let wheelAcc = 0;
crate.addEventListener('wheel', (e) => {
  e.preventDefault();
  if (state.view !== 'library') return;
  const raw = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
  const d = e.deltaMode === 1 ? raw * 40 : raw;
  wheelAcc += d;
  const step = 70;
  while (Math.abs(wheelAcc) >= step) {
    const dir = Math.sign(wheelAcc);
    state.target = clamp(state.target + dir, 0, state.albums.length - 1);
    wheelAcc -= dir * step;
  }
  rememberPosition();
  kick();
}, { passive: false });

// drag / swipe
let dragging = false, moved = false, startX = 0, startTarget = 0, downSleeve = null, lastX = 0, lastT = 0, vel = 0;
crate.addEventListener('pointerdown', (e) => {
  if (e.button !== 0) return;
  dragging = true; moved = false;
  startX = lastX = e.clientX; lastT = performance.now(); vel = 0;
  startTarget = state.target;
  downSleeve = e.target.closest('.sleeve');
  crate.setPointerCapture(e.pointerId);
});
crate.addEventListener('pointermove', (e) => {
  if (!dragging) return;
  const dx = e.clientX - startX;
  if (Math.abs(dx) > 6) { moved = true; crate.classList.add('dragging'); }
  if (!moved) return;
  const now = performance.now();
  vel = (e.clientX - lastX) / Math.max(1, now - lastT);
  lastX = e.clientX; lastT = now;
  state.target = clamp(startTarget - dx / (state.size * 0.32), 0, state.albums.length - 1);
  kick();
});
function endDrag() {
  if (!dragging) return;
  dragging = false;
  crate.classList.remove('dragging');
  if (moved) {
    go(state.target - vel * 6); // gentle flick
  } else if (downSleeve) {
    const i = Number(downSleeve.dataset.index);
    if (i === Math.round(state.target)) openAlbum(state.albums[i]);
    else go(i);
  }
  downSleeve = null;
}
crate.addEventListener('pointerup', endDrag);
crate.addEventListener('pointercancel', endDrag);

/* ---------------- library loading ---------------- */

let libraryRequest = 0;
async function loadAlbums() {
  const request = ++libraryRequest;
  const albums = await invoke('get_albums');
  if (request !== libraryRequest) return;
  const byId = new Map(albums.map(a => [a.id, a]));
  const represented = new Set();
  for (const a of albums) {
    const artists = (a.members || []).map(id => {
      represented.add(id);
      return byId.get(id)?.artist || '';
    });
    a.search = [a.artist, a.title, a.year, ...artists].join(' ').toLowerCase();
  }
  artistAlbums = albums.filter(a => !a.members?.length);
  titleAlbums = albums.filter(a => !represented.has(a.id));
  applyLibraryOrder();
}
function applyLibraryOrder() {
  const previous = focused();
  const keepId = previous?.id ?? savedPosition?.id;
  const fallback = state.albums.length ? Math.round(state.target) : savedPosition?.index;
  state.albums = indexAlbums(indexMode === 'album' ? titleAlbums : artistAlbums);
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
  closeSearch();
  if (indexMode === 'artist') preferredMember = focused()?.id;
  indexMode = indexMode === 'artist' ? 'album' : 'artist';
  localStorage.setItem('muso.indexMode', indexMode);
  applyLibraryOrder();
});

async function boot() {
  layout();
  const folders = await invoke('get_folders');
  if (!folders.length) {
    showOnboard();
    return;
  }
  await loadAlbums();
  if (state.albums.length) showLibrary();
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
  await loadAlbums();
  if (state.albums.length) {
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
        button.title = 'Download and install update';
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
function closeMenu() { show($('menu'), false); }
$('menuBtn').addEventListener('click', openMenu);
$('menu').querySelector('.menu-scrim').addEventListener('click', closeMenu);
$('menu').querySelectorAll('[data-action]').forEach((b) =>
  b.addEventListener('click', () => {
    closeMenu();
    const act = b.dataset.action;
    if (act === 'search') openSearch();
    if (act === 'settings') openSettings();
    if (act === 'eq') openEQ(false);
    if (act === 'fullscreen') toggleFullscreen();
    if (act === 'ai') openAi();
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
  const q = $('searchInput').value.trim().toLowerCase();
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
    s.title = f;
    const x = document.createElement('button');
    x.textContent = '×';
    x.title = 'Remove';
    x.addEventListener('click', async () => {
      renderFolders(await invoke('remove_folder', { path: f }));
      invoke('scan_library');
    });
    li.append(s, x);
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
  await invoke('set_setting', { key: 'openai_key', value: e.target.value });
  toast(e.target.value.trim() ? 'Key saved' : 'Saved key removed');
}
document.querySelectorAll('.openai-keys').forEach((button) => {
  button.addEventListener('click', () => invoke('open_openai_keys').catch((e) => toast(String(e))));
});
$('keyInput').addEventListener('change', saveKey);
$('keyInput2').addEventListener('change', saveKey);

async function openSettings() {
  renderFolders(await invoke('get_folders'));
  await loadKey();
  show($('settings'));
}

const filterSelect = $('imageFilter');
const rippleToggle = $('pointerRipples');
const strengthInput = $('effectStrength');
const spectrum = createSpectrumView(invoke, listen);
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
let waveFrame = 0;
let wave;
filterSelect.value = localStorage.getItem('muso.imageFilter') || 'light';
if (!filterSelect.value) filterSelect.value = 'light';
rippleToggle.checked = localStorage.getItem('muso.pointerRipples') !== 'off';
strengthInput.value = localStorage.getItem('muso.effectStrength') ?? '40';
function applyEffects() {
  $('album').dataset.filter = filterSelect.value;
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
      !album.classList.contains('showing-atmos') || e.target.closest('button, .popover, .controls, .al-track')) return;
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

function applyCover(id, file) {
  const updated = new Set();
  for (const a of [...artistAlbums, ...titleAlbums, state.viewAlbum, state.playing?.album]) {
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
  if (updated.has(state.viewAlbum?.id)) {
    for (const el of [$('coverImg'), $('bgArt')]) {
      el.src = coverUrl(state.viewAlbum, 'art');
      show(el);
    }
  }
}

async function findCovers(albums, single = false) {
  if (coverSearchRunning) return;
  coverSearchRunning = true;
  stopCoverSearch = false;
  const button = $('findCovers');
  button.textContent = 'Stop search';
  $('findAlbumCover').disabled = true;
  let checked = 0, updated = 0, failed = 0, consecutiveErrors = 0;
  let lastError = '';
  try {
    for (const album of albums) {
      if (stopCoverSearch) break;
      $('coverProgress').textContent = `Checking ${checked + 1} of ${albums.length} · ${album.title}`;
      try {
        const file = await invoke('upgrade_cover', { albumId: album.id });
        if (file) { applyCover(album.id, file); updated++; }
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
    if (single && !failed) summary = updated ? 'Cover updated' : 'No higher-resolution match found';
    if (failed) summary += ` · ${failed} failed. ${lastError}`;
    $('coverProgress').textContent = summary;
    if (single || $('settings').classList.contains('hidden')) toast(summary, 4500);
  } finally {
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
    findCovers(state.albums.slice());
  }
});
$('findAlbumCover').addEventListener('click', () => {
  show($('albumMenu'), false);
  if (state.viewAlbum) {
    toast('Looking for a better cover…');
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
};
function albumStyle(id) {
  if ($('autoStyle').checked) return 'auto';
  const saved = localStorage.getItem('muso.style.' + id);
  return Object.hasOwn(styleNames, saved) ? saved : 'cinematic';
}
let styleAlbum;
async function showStyle(id) {
  styleAlbum = id;
  const style = albumStyle(id);
  const auto = style === 'auto';
  for (const select of [$('albumStyle'), $('aiStyle')]) {
    select.disabled = auto;
    select.options[0].textContent = 'AI choice';
    select.value = style;
  }
  $('genLabel').textContent = auto ? 'Generate with AI style' : 'Generate ' + styleNames[style] + ' images';
  if (auto && id) {
    const name = await invoke('get_setting', { key: 'atmos_auto_style.' + id });
    if (name && styleAlbum === id && $('autoStyle').checked) {
      for (const select of [$('albumStyle'), $('aiStyle')]) select.options[0].textContent = name;
    }
  }
}
for (const select of [$('albumStyle'), $('aiStyle')]) {
  const automatic = new Option('AI choice', 'auto');
  automatic.hidden = true;
  select.add(automatic);
  for (const [id, label] of Object.entries(styleNames)) select.add(new Option(label, id));
  select.addEventListener('change', () => {
    const a = state.viewAlbum || focused();
    if (!a) return;
    localStorage.setItem('muso.style.' + a.id, select.value);
    showStyle(a.id);
    if (state.viewAlbum) startAtmospheres(a.id);
  });
}
$('autoStyle').checked = localStorage.getItem('muso.autoStyle') === '1';
$('autoStyle').addEventListener('change', () => {
  localStorage.setItem('muso.autoStyle', $('autoStyle').checked ? '1' : '0');
  showStyle((state.viewAlbum || focused())?.id);
  if (state.viewAlbum) startAtmospheres(state.viewAlbum.id);
});
async function openAi() {
  await loadKey();
  $('atmosToggle').checked = atmosEnabled();
  $('researchToggle').checked = researchEnabled();
  showStyle((state.viewAlbum || focused())?.id);
  show($('ai'));
}
const researchEnabled = () => localStorage.getItem('muso.research') === '1';
$('researchToggle').addEventListener('change', (e) => localStorage.setItem('muso.research', e.target.checked ? '1' : '0'));
function setAtmosEnabled(enabled) {
  localStorage.setItem('muso.atmos', enabled ? '1' : '0');
  $('atmosToggle').checked = enabled;
  if (state.viewAlbum) startAtmospheres(state.viewAlbum.id, true);
}
$('atmosToggle').addEventListener('change', (e) => setAtmosEnabled(e.target.checked));
$('aiGenerateBtn').addEventListener('click', () => {
  const a = state.viewAlbum || focused();
  show($('ai'), false);
  if (a) generate(a);
});

document.querySelectorAll('.modal').forEach((m) => {
  m.addEventListener('click', (e) => { if (e.target === m) show(m, false); });
  m.querySelector('.close').addEventListener('click', () => show(m, false));
});

/* ---------------- playback ---------------- */

let playbackId = 0;
async function playIndex(i) {
  const p = state.playing;
  if (!p) return;
  if (i >= p.tracks.length) {
    // The record has ended: lift the needle.
    await invoke('stop');
    Object.assign(p, { index: 0, paused: true, ended: true, introDone: false });
    updatePlayUi();
    return;
  }
  const changingTrack = p.playbackId && state.viewAlbum?.id === p.album.id;
  Object.assign(p, { index: Math.max(0, i), paused: false, ended: false, introDone: false, playbackId: ++playbackId });
  updateAtmosView();
  if (changingTrack && !reducedMotion.matches) {
    coverFade?.cancel();
    coverFade = $('cover').animate([{ opacity: 0 }, { opacity: 1 }], { duration: 1450, easing: 'ease-in-out' });
  }
  await invoke('play_track', { path: p.tracks[p.index].path, playbackId: p.playbackId });
  updatePlayUi();
}
async function togglePlay() {
  const p = state.playing;
  if (!p) return;
  if (p.ended) return playIndex(0);
  p.paused = !p.paused;
  await invoke(p.paused ? 'pause' : 'resume');
  updatePlayUi();
}
const next = () => state.playing && playIndex(state.playing.index + 1);
const prev = () => state.playing && playIndex(state.playing.index - 1);

listen('track-ended', () => next());
listen('cover-intro-ended', ({ payload }) => {
  const p = state.playing;
  if (!p || p.ended || p.playbackId !== payload) return;
  p.introDone = true;
  updateAtmosView();
});
listen('playback-error', ({ payload }) => {
  toast(`Skipped · ${payload}`, 3500);
  if (state.playing && !state.playing.ended) next();
});

let coverFade;
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
      const label = p.album.members?.length > 1 && track.artist ? track.artist + ' — ' + track.title : track.title;
      button.append(number, document.createTextNode(label));
      button.title = label;
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

function updatePlayUi() {
  updateAtmosView();
  const p = state.playing;
  const np = $('nowPlaying');
  show(np, !!p);
  if (!p) return;
  np.classList.toggle('paused', p.paused);
  $('npText').textContent = `${p.album.artist} — ${p.album.title}`;
  if (state.viewAlbum && state.viewAlbum.id === p.album.id) {
    $('album').classList.toggle('is-paused', p.paused);
    updateTrackRail(p);
  }
}

$('playBtn').addEventListener('click', togglePlay);
$('nextBtn').addEventListener('click', next);
$('prevBtn').addEventListener('click', prev);
$('nowPlaying').addEventListener('click', () => state.playing && openAlbum(state.playing.album));

/* ---------------- album view ---------------- */

const proposedCover = $('proposedCover');
let proposal = null;
let albumRequest = 0;

function clearProposal() {
  const previous = proposal;
  proposal = null;
  previous?.animation?.cancel();
  if (previous?.drag && proposedCover.hasPointerCapture(previous.drag.id)) {
    proposedCover.releasePointerCapture(previous.drag.id);
  }
  proposedCover.classList.remove('dragging');
  proposedCover.removeAttribute('style');
  show(proposedCover, false);
}

function openAlbum(a) {
  ++albumRequest;
  clearProposal();
  const playing = state.playing;
  if (playing?.album.id === a.id) return displayAlbum(a);
  if (!playing || playing.paused || playing.ended) return startAlbum(a);

  proposal = { album: a, drag: null, busy: false };
  const preview = $('proposedImg');
  preview.onerror = () => show(preview, false);
  show(preview, a.art);
  if (a.art) preview.src = coverUrl(a, 'art');
  else preview.removeAttribute('src');
  proposedCover.style.setProperty('--blank', blankColor(a.id));
  const hint = 'Swipe right to play ' + a.artist + ' — ' + a.title;
  proposedCover.title = hint;
  proposedCover.setAttribute('aria-label', hint);
  proposedCover.querySelector('.swipe-hint').textContent = a.art ? 'Swipe right →' : a.title + ' →';
  displayAlbum(playing.album);
  show(proposedCover);
  proposedCover.focus({preventScroll:true});
  if (!reducedMotion.matches) {
    proposal.animation = proposedCover.animate([
      { transform: 'translateX(24%) scale(.94) rotate(0deg)' },
      { transform: 'translateX(48%) scale(.94) rotate(5deg)', offset: .55 },
      { transform: 'translateX(46%) scale(.94) rotate(0deg)' },
    ], { duration: 740, easing: 'ease-in-out' });
  }
}

proposedCover.addEventListener('pointerdown', e => {
  if (!proposal || proposal.busy || proposal.drag || !e.isPrimary || e.button !== 0) return;
  proposal.animation?.cancel();
  proposal.drag = { id: e.pointerId, x: e.clientX, y: e.clientY };
  proposedCover.setPointerCapture(e.pointerId);
  proposedCover.classList.add('dragging');
});
proposedCover.addEventListener('pointermove', e => {
  const drag = proposal?.drag;
  if (!drag || drag.id !== e.pointerId) return;
  const pull = clamp(e.clientX - drag.x, 0, proposedCover.offsetWidth * .5);
  proposedCover.style.setProperty('--pull', pull + 'px');
  proposedCover.style.setProperty('--tilt', Math.min(7, pull / 18) + 'deg');
});
function endProposalDrag(e) {
  const drag = proposal?.drag;
  if (!drag || drag.id !== e.pointerId) return;
  proposal.drag = null;
  const dx = e.clientX - drag.x;
  const threshold = clamp(proposedCover.offsetWidth * .14, 36, 80);
  if (e.type === 'pointerup' && dx >= threshold && dx > Math.abs(e.clientY - drag.y)) {
    startAlbum(proposal.album);
    return;
  }
  proposedCover.classList.remove('dragging');
  proposedCover.style.removeProperty('--pull');
  proposedCover.style.removeProperty('--tilt');
}
for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
  proposedCover.addEventListener(type, endProposalDrag);
}
proposedCover.addEventListener('keydown', e => {
  if (e.key === 'Enter' || e.key === ' ') e.stopPropagation();
});
proposedCover.addEventListener('click', e => {
  if (e.detail === 0 && proposal && !proposal.busy) startAlbum(proposal.album);
});

async function startAlbum(a) {
  const offer = proposal;
  if (offer?.busy) return;
  if (offer) offer.busy = true;
  const request = ++albumRequest;
  try {
    const tracks = await invoke('get_tracks', {albumId: a.id});
    if (request !== albumRequest) return;
    if (!tracks.length) throw new Error('No tracks available for this album.');
    if (offer && !reducedMotion.matches) {
      proposedCover.style.zIndex = '2';
      offer.animation = proposedCover.animate([
        { transform: getComputedStyle(proposedCover).transform },
        { transform: 'translateX(0) scale(1) rotate(0deg)' },
      ], {duration: 320, easing: 'cubic-bezier(.2,.75,.25,1)', fill: 'forwards'});
      await offer.animation.finished;
    }
    if (request !== albumRequest) return;
    clearProposal();
    state.playing = { album: a, tracks, index: 0, paused: false, ended: false };
    displayAlbum(a);
    await playIndex(0);
  } catch (e) {
    if (request !== albumRequest) return;
    clearProposal();
    updatePlayUi();
    toast(String(e), 4500);
  }
}

function displayAlbum(a) {
  closeSearch();
  closeMenu();
  state.view = 'album';
  state.viewAlbum = a;
  const view = $('album');
  view.classList.remove('leaving', 'idle');
  view.style.setProperty('--blank', blankColor(a.id));
  const cover = $('coverImg');
  const bg = $('bgArt');
  if (a.art) {
    cover.src = bg.src = coverUrl(a, 'art');
    show(cover); show(bg);
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
  ++albumRequest;
  clearProposal();
  const view = $('album');
  view.classList.add('leaving');
  stopAtmospheres();
  state.view = 'library';
  state.viewAlbum = null;
  show($('menuBtn'));
  setTimeout(() => { if (state.view === 'library') show(view, false); view.classList.remove('leaving'); }, 450);
}
$('backBtn').addEventListener('click', closeAlbum);

// Calm mode: chrome fades away when the mouse rests.
let idleTimer;
function poke() {
  if ($('album').classList.contains('idle')) centerTrack();
  $('album').classList.remove('idle');
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (state.view === 'album' && !proposal && $('albumMenu').classList.contains('hidden')) $('album').classList.add('idle');
  }, 3500);
}
$('album').addEventListener('mousemove', poke);
$('album').addEventListener('click', (e) => {
  poke();
  if (!e.target.closest('#albumMenu, #albumMenuBtn')) show($('albumMenu'), false);
});
const playerWindow = window.__TAURI__.window.getCurrentWindow();
function showFullscreenAction(fullscreen) {
  document.querySelectorAll('.fullscreen-label').forEach(el => { el.textContent = fullscreen ? 'Exit fullscreen' : 'Enter fullscreen'; });
  document.querySelectorAll('.fullscreen-icon').forEach(el => el.setAttribute('href', 'icons/lucide.svg#' + (fullscreen ? 'minimize' : 'maximize')));
}
async function toggleFullscreen() {
  try {
    const enter = !(await playerWindow.isFullscreen());
    await playerWindow.setFullscreen(enter);
    showFullscreenAction(enter);
  } catch (e) { toast(String(e)); }
}
$('albumMenuBtn').addEventListener('click', async () => {
  show($('albumMenu'), $('albumMenu').classList.contains('hidden'));
  showFullscreenAction(await playerWindow.isFullscreen());
});
$('fullscreenBtn').addEventListener('click', () => { show($('albumMenu'), false); toggleFullscreen(); });
function openEQ(fromPlayer) {
  if (fromPlayer) show($('albumMenu'), false);
  $('eq').classList.toggle('from-player', fromPlayer);
  show($('eq'));
  $('spectrumToggle').focus();
}
$('albumEQ').addEventListener('click', () => openEQ(true));
$('eq').querySelector('.close').addEventListener('click', () => {
  if (!$('eq').classList.contains('from-player')) { openMenu(); $('menu').querySelector('[data-action="eq"]').focus(); }
  else { poke(); $('albumMenuBtn').focus(); }
});
$('openImageFolder').addEventListener('click', async () => {
  show($('albumMenu'), false);
  if (!state.viewAlbum) return;
  try { await invoke('open_image_folder', { albumId: state.viewAlbum.id }); }
  catch (e) { toast(String(e), 5000); }
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

let atmosTimer = null;
let atmosToken = 0;
let readyAlbum = null;
let playlist = null;
function updateAtmosView() {
  const p = state.playing;
  const reveal = !proposal && p?.introDone && readyAlbum === p.album.id && state.viewAlbum?.id === readyAlbum;
  $('album').classList.toggle('showing-atmos', !!reveal);
  spectrum.setPlayback(!!reveal && !p.paused, p?.playbackId);
  if (!reveal) stopWave();
}
async function startAtmospheres(albumId, announce = false) {
  stopAtmospheres();
  const token = ++atmosToken;
  if (!atmosEnabled()) return;
  const style = albumStyle(albumId);
  try {
    // Preview the chosen look on a saved set until this album has images in that style.
    for (const candidate of new Set([style, ...Object.keys(styleNames), 'auto'])) {
      const files = await invoke('get_atmospheres', { albumId, style: candidate });
      if (token !== atmosToken) return;
      if (files.length) {
        setPaintings(albumId, candidate, files);
        return;
      }
    }
  }
  catch (e) { if (token === atmosToken) toast(String(e), 6000); return; }
  if (announce) toast('No saved images for this album. Choose Generate Visual Atmospheres.', 5500);
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

function setPaintings(albumId, style, files) {
  if (!files.length) return;
  ++atmosToken; // Discard any library lookup that began before this set arrived.
  if (playlist?.albumId === albumId && playlist.style === style && playlist.first === files[0]) {
    playlist.files = files.filter(file => !playlist.failed.has(file));
    schedulePainting();
    return;
  }
  clearTimeout(atmosTimer);
  playlist = { albumId, style, first: files[0], files, failed: new Set() };
  loadPainting(playlist, files[0]);
}

function loadPainting(set, file) {
  set.pending = true;
  const other = $('atmosA').classList.contains('on') ? $('atmosA') : $('atmosB');
  const img = other === $('atmosA') ? $('atmosB') : $('atmosA');
  img.onload = () => {
    if (set !== playlist) return;
    set.pending = false;
    set.shown = file;
    set.since = performance.now();
    readyAlbum = set.albumId;
    $('album').dataset.preview = set.style === albumStyle(set.albumId) ? '' : albumStyle(set.albumId);
    img.classList.add('on');
    other.classList.remove('on');
    updateAtmosView();
    $('showAtmosBtn').setAttribute('aria-pressed', 'true');
    schedulePainting();
  };
  img.onerror = () => {
    if (set !== playlist) return;
    set.pending = false;
    set.failed.add(file);
    set.files = set.files.filter(f => f !== file);
    if (set.shown) { schedulePainting(); return; }
    if (set.files.length) { loadPainting(set, set.files[0]); return; }
    stopAtmospheres();
    toast('Saved atmosphere images could not be opened.', 5000);
  };
  img.src = asset(file);
}
function stopAtmospheres() {
  atmosToken++;
  playlist = null;
  clearTimeout(atmosTimer);
  atmosTimer = null;
  readyAlbum = null;
  updateAtmosView();
  $('showAtmosBtn').setAttribute('aria-pressed', 'false');
  for (const id of ['atmosA', 'atmosB']) {
    const img = $(id);
    img.classList.remove('on');
    img.onload = img.onerror = null;
    img.removeAttribute('src');
  }
}

const generating = new Set();
function paintingReady({ albumId, style, files }) {
  if (state.viewAlbum?.id === albumId && atmosEnabled() && albumStyle(albumId) === style) {
    setPaintings(albumId, style, files);
  }
}
listen('atmos-image-ready', ({ payload }) => {
  paintingReady(payload);
  if (payload.files.length === 1 && styleAlbum === payload.albumId) showStyle(payload.albumId);
});
async function generate(a) {
  const key = await invoke('get_setting', { key: 'openai_key' });
  if (!key && !(await invoke('has_environment_key'))) { openAi(); toast('Add an OpenAI API key first'); return; }
  if (generating.has(a.id)) return;
  generating.add(a.id);
  toast(`Composing atmospheres for ${a.title}…`);
  try {
    const style = albumStyle(a.id);
    const files = await invoke('generate_atmospheres', { albumId: a.id, research: researchEnabled(), style });
    paintingReady({ albumId: a.id, style, files });
    toast(`${files.length} atmosphere image(s) saved · ${a.title}`);
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
  const modalOpen = [...document.querySelectorAll('.modal')].some((m) => !m.classList.contains('hidden'));
  if (e.key === 'F11') {
    e.preventDefault();
    if (!e.repeat) toggleFullscreen();
    return;
  }
  if (e.key === 'Escape' && !$('eq').classList.contains('hidden')) {
    $('eq').querySelector('.close').click();
    return;
  }
  if (e.target.matches('input, select')) return;
  if (e.key === 'Escape') {
    if (modalOpen) document.querySelectorAll('.modal').forEach((m) => show(m, false));
    else if (!$('menu').classList.contains('hidden')) closeMenu();
    else if (!$('search').classList.contains('hidden')) closeSearch();
    else if (state.view === 'album') closeAlbum();
    return;
  }
  if (modalOpen || !$('menu').classList.contains('hidden')) return;
  if (e.target === $('noteAction') && (e.key === ' ' || e.key === 'Enter')) return;
  if (libraryIndex.contains(e.target) && (e.key === 'Enter' || e.key === ' ')) return;
  if (e.key === 'f' && state.view === 'album') {
    toggleFullscreen();
    e.preventDefault();
    return;
  }
  if (e.key === ' ') { togglePlay(); e.preventDefault(); return; }

  if (state.view === 'album') {
    if (e.key === 'ArrowRight') next();
    else if (e.key === 'ArrowLeft') prev();
    else if (e.key === 'Backspace') closeAlbum();
    poke();
    return;
  }
  if (state.view !== 'library' || !state.albums.length) return;
  const t = Math.round(state.target);
  if (e.key === 'ArrowRight' || e.key === 'ArrowDown') go(t + 1);
  else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') go(t - 1);
  else if (e.key === 'PageDown') go(t + 10);
  else if (e.key === 'PageUp') go(t - 10);
  else if (e.key === 'Home') go(0);
  else if (e.key === 'End') go(state.albums.length - 1);
  else if (e.key === 'Enter') openAlbum(focused());
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
