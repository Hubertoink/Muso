'use strict';

function createLibrarySaver(isTvLibrary) {
  const layer = document.getElementById('librarySaver');
  const slots = [...layer.querySelectorAll('img')];
  const blockers = [...document.querySelectorAll('#menu, #search, .modal, dialog')];
  let eligible = false, idle = false, timer;
  let picture = '', generation = 0;

  function hide() {
    ++generation;
    layer.classList.remove('on');
  }

  async function reveal() {
    if (!idle || !picture) return;
    const request = ++generation;
    let img = slots.find(slot => slot.getAttribute('src') === picture);
    if (!img) {
      img = slots.find(slot => !slot.classList.contains('on'));
      img.src = picture;
    }
    try { await img.decode(); } catch {
      if (request === generation) hide();
      return;
    }
    if (request !== generation) return;
    for (const slot of slots) slot.classList.toggle('on', slot === img);
    layer.classList.add('on');
  }

  function restart() {
    clearTimeout(timer);
    idle = false;
    hide();
    if (eligible) timer = setTimeout(() => {
      idle = true;
      reveal();
    }, 60000);
  }

  function sync() {
    const allowed = isTvLibrary() && !document.hidden && !blockers.some(el =>
      el.tagName === 'DIALOG' ? el.open : !el.classList.contains('hidden'));
    if (eligible === allowed) return;
    eligible = allowed;
    restart();
  }

  const observer = new MutationObserver(sync);
  for (const el of [...blockers, document.getElementById('library'), document.getElementById('album')]) {
    observer.observe(el, {attributes: true, attributeFilter: ['class', 'open']});
  }
  document.addEventListener('visibilitychange', sync);

  function activity(e) {
    if (!eligible) return;
    if (layer.classList.contains('on')) {
      e.preventDefault();
      e.stopImmediatePropagation();
    }
    restart();
    // A finger or a pressed mouse button resting on a sleeve is still browsing.
    if (e.buttons || e.type === 'pointerdown') clearTimeout(timer);
  }
  for (const type of ['pointermove', 'pointerdown', 'pointerup', 'pointercancel', 'keydown', 'wheel']) {
    window.addEventListener(type, activity, {capture: true, passive: false});
  }
  return {
    sync,
    setPicture(url) {
      if (picture === url) return;
      picture = url;
      if (url) reveal();
      else hide();
    },
  };
}
