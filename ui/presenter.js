'use strict';
const channel = new BroadcastChannel('muso-presenter');
const stage = document.querySelector('main');
const caption = document.querySelector('aside');
const failed = new Set();
let current = null, desired = '', shown = '', timer = null;
function candidates() {
  return (current?.files || []).filter(url => !failed.has(url));
}
function schedule() {
  clearTimeout(timer);
  if (!current || current.paused || candidates().length < 2) return;
  timer = setTimeout(() => {
    const files = candidates();
    display(files[(files.indexOf(shown) + 1) % files.length]);
  }, 45000);
}
async function display(url) {
  desired = url;
  if (!url) { shown = ''; stage.replaceChildren(); clearTimeout(timer); return; }
  if (url === shown) { schedule(); return; }
  const image = new Image();
  image.alt = '';
  image.src = url;
  try { await image.decode(); } catch {
    if (desired !== url) return;
    failed.add(url);
    display(candidates()[0] || (!failed.has(current.cover) && current.cover) || '');
    return;
  }
  if (desired !== url) return;
  shown = url;
  const previous = stage.lastElementChild;
  stage.append(image);
  image.getBoundingClientRect();
  image.classList.add('visible');
  if (previous) { previous.classList.remove('visible'); setTimeout(() => previous.remove(), 1550); }
  schedule();
}
channel.onmessage = ({data}) => {
  if (!data || typeof data !== 'object') return;
  const changed = current?.track !== data.track;
  const pausedChanged = current?.paused !== data.paused;
  const filesChanged = JSON.stringify(current?.files) !== JSON.stringify(data.files);
  if (changed || filesChanged) failed.clear();
  current = data;
  document.querySelector('#artist').textContent = data.artist || '';
  document.querySelector('#title').textContent = data.title || '';
  caption.classList.toggle('visible', !!data.track && data.intro);
  const files = candidates();
  const url = files.includes(desired) ? desired : files[0] || (!failed.has(data.cover) && data.cover) || '';
  if (url !== desired) display(url);
  else if (changed || pausedChanged || filesChanged) schedule();
};
addEventListener('keydown', e => { if (e.key === 'Escape') channel.postMessage('close'); });
channel.postMessage('ready');
