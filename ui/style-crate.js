'use strict';

function createCardCrate(dialog, choose, prepare) {
  const canvas = dialog.querySelector('.card-canvas');
  const cards = canvas.children;
  const previous = dialog.querySelector('.prev-card'), next = dialog.querySelector('.next-card');
  const caption = dialog.querySelector('.card-caption');
  const motion = { pos: 0, target: 0 };
  let size = 0;
  let frame = 0, wheel = 0;

  function paint() {
    for (let i = 0; i < cards.length; i++) {
      const sleeve = cards[i];
      sleeve.hidden = Math.abs(i - motion.pos) >= WINDOW;
      if (!sleeve.hidden) positionSleeve(sleeve, i - motion.pos, size);
    }
    const index = Math.round(motion.target);
    const label = cards[index].getAttribute('aria-label') || cards[index].value;
    if (caption.textContent !== label) caption.textContent = label;
    previous.disabled = index === 0;
    next.disabled = index === cards.length - 1;
  }

  function navigate(index, time = defaultSettleTime) {
    cancelAnimationFrame(frame);
    motion.target = clamp(Math.round(index), 0, cards.length - 1);
    let stamp = performance.now();
    function tick(now) {
      frame = 0;
      const diff = motion.target - motion.pos;
      motion.pos = reducedMotion.matches || Math.abs(diff) * size < 0.25
        ? motion.target : motion.pos + diff * -Math.expm1(-(now - stamp) / time);
      paint();
      stamp = now;
      if (motion.pos !== motion.target) frame = requestAnimationFrame(tick);
    }
    tick(stamp);
  }

  previous.onclick = () => navigate(motion.target - 1);
  next.onclick = () => navigate(motion.target + 1);
  dialog.querySelector('.use-card').onclick = () => choose(cards[Math.round(motion.target)].value);
  dialog.addEventListener('keydown', e => {
    if (e.key === 'ArrowLeft') navigate(motion.target - 1);
    else if (e.key === 'ArrowRight') navigate(motion.target + 1);
    else if (e.key === 'Home') navigate(0);
    else if (e.key === 'End') navigate(cards.length - 1);
    else if (canvas.contains(e.target) && ['Enter', ' '].includes(e.key)) choose(cards[Math.round(motion.target)].value);
    else return;
    e.preventDefault();
    e.stopPropagation();
  });
  canvas.addEventListener('wheel', e => {
    e.preventDefault();
    if (gestures.active) return;
    const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    wheel += delta * (e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? canvas.clientWidth : 1);
    const steps = Math.trunc(wheel / 70);
    if (steps) { wheel -= steps * 70; navigate(motion.target + steps); }
  }, { passive: false });

  const gestures = bindSleeveGestures(canvas, motion, {
    spacing: () => size * 0.66,
    lastIndex: () => cards.length - 1,
    hold: () => { cancelAnimationFrame(frame); frame = 0; wheel = 0; paint(); },
    render() {
      if (!frame && dialog.open) frame = requestAnimationFrame(() => { frame = 0; paint(); });
    },
    navigate,
    activate: sleeve => choose(sleeve.value),
  });
  // Pointer taps are handled on release. Keep non-pointer button activation accessible.
  canvas.addEventListener('click', e => {
    if (e.detail !== 0 || e.pointerType) return;
    const sleeve = e.target.closest('.sleeve');
    if (!sleeve) return;
    const index = Number(sleeve.dataset.index);
    if (index === Math.round(motion.pos) && index === Math.round(motion.target)) choose(cards[index].value);
    else navigate(index);
  });

  function measure() {
    if (!dialog.open || !canvas.firstElementChild) return;
    size = canvas.querySelector('.sleeve:not([hidden])').offsetWidth;
    paint();
  }
  window.addEventListener('resize', measure);
  function close() {
    if (!dialog.open || dialog.classList.contains('closing')) return;
    dialog.classList.add('closing');
    dialog.inert = true;
    // Keep the native backdrop in the top layer until its fade has finished.
    const fades = dialog.getAnimations().filter(animation => animation.transitionProperty === 'opacity');
    if (fades.length) Promise.allSettled(fades.map(animation => animation.finished)).then(() => dialog.close());
    else dialog.close();
  }
  dialog.addEventListener('cancel', e => { e.preventDefault(); close(); });
  dialog.querySelector('header button').addEventListener('click', close);
  dialog.addEventListener('click', e => {
    if (e.target !== dialog) return;
    const box = dialog.getBoundingClientRect();
    if (e.clientX < box.left || e.clientX > box.right || e.clientY < box.top || e.clientY > box.bottom) close();
  });
  dialog.addEventListener('close', () => {
    cancelAnimationFrame(frame);
    frame = 0;
    gestures.reset();
  });
  return { close, open(selected) {
    dialog.inert = false;
    dialog.classList.remove('closing');
    dialog.showModal();
    if (!cards.length && prepare) prepare(canvas);
    motion.pos = motion.target = Math.max(0, [...cards].findIndex(card => card.value === selected));
    for (const [i, card] of [...cards].entries()) {
      card.dataset.index = i;
      card.tabIndex = -1;
      card.setAttribute('aria-pressed', String(card.value === selected));
    }
    wheel = 0;
    measure();
    canvas.focus({ preventScroll: true });
  } };
}

function createStyleCrate(dialog, names, choose) {
  return createCardCrate(dialog, choose, canvas => {
    for (const [id, name] of Object.entries(names)) {
      const sleeve = document.createElement('button');
      sleeve.type = 'button';
      sleeve.className = 'sleeve';
      sleeve.value = sleeve.dataset.style = id;
      sleeve.setAttribute('aria-label', name);
      const img = new Image();
      img.alt = '';
      img.draggable = false;
      img.src = 'style-previews/' + id + '.webp';
      sleeve.append(img);
      canvas.append(sleeve);
    }
  });
}
