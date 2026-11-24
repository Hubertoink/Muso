'use strict';

function createStyleCrate(dialog, names, choose) {
  const canvas = $('styleCanvas');
  const styles = Object.keys(names);
  const last = styles.length - 1;
  const motion = { pos: 0, target: 0 };
  let size = 0;
  let frame = 0, wheel = 0;

  function paint() {
    for (let i = 0; i < styles.length; i++) {
      const sleeve = canvas.children[i];
      sleeve.hidden = Math.abs(i - motion.pos) >= WINDOW;
      if (!sleeve.hidden) positionSleeve(sleeve, i - motion.pos, size);
    }
    const index = Math.round(motion.target);
    const label = names[styles[index]];
    if ($('styleCaption').textContent !== label) $('styleCaption').textContent = label;
    $('prevStyle').disabled = index === 0;
    $('nextStyle').disabled = index === last;
  }

  function navigate(index, time = defaultSettleTime) {
    cancelAnimationFrame(frame);
    motion.target = clamp(Math.round(index), 0, last);
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

  $('prevStyle').onclick = () => navigate(motion.target - 1);
  $('nextStyle').onclick = () => navigate(motion.target + 1);
  $('useStyle').onclick = () => choose(styles[Math.round(motion.target)]);
  dialog.addEventListener('keydown', e => {
    if (e.key === 'ArrowLeft') navigate(motion.target - 1);
    else if (e.key === 'ArrowRight') navigate(motion.target + 1);
    else if (e.key === 'Home') navigate(0);
    else if (e.key === 'End') navigate(last);
    else if (canvas.contains(e.target) && ['Enter', ' '].includes(e.key)) choose(styles[Math.round(motion.target)]);
    else return;
    e.preventDefault();
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
    lastIndex: () => last,
    hold: () => { cancelAnimationFrame(frame); frame = 0; wheel = 0; paint(); },
    render() {
      if (!frame && dialog.open) frame = requestAnimationFrame(() => { frame = 0; paint(); });
    },
    navigate,
    activate: sleeve => choose(sleeve.dataset.style),
  });
  // Pointer taps are handled on release. Keep non-pointer button activation accessible.
  canvas.addEventListener('click', e => {
    if (e.detail !== 0 || e.pointerType) return;
    const sleeve = e.target.closest('[data-style]');
    if (!sleeve) return;
    const index = Number(sleeve.dataset.index);
    if (index === Math.round(motion.pos) && index === Math.round(motion.target)) choose(styles[index]);
    else navigate(index);
  });

  function measure() {
    if (!dialog.open || !canvas.firstElementChild) return;
    size = Math.min(window.innerHeight * 0.43, window.innerWidth * 0.44, 380);
    canvas.style.setProperty('--size', size + 'px');
    paint();
  }
  window.addEventListener('resize', measure);
  dialog.addEventListener('close', () => {
    cancelAnimationFrame(frame);
    frame = 0;
    gestures.reset();
  });
  return { open(selected) {
    if (!canvas.childElementCount) {
      for (const id of styles) {
        const sleeve = document.createElement('button');
        sleeve.type = 'button';
        sleeve.className = 'sleeve';
        sleeve.tabIndex = -1;
        sleeve.dataset.style = id;
        sleeve.dataset.index = canvas.childElementCount;
        sleeve.setAttribute('aria-label', names[id]);
        const img = new Image();
        img.alt = '';
        img.draggable = false;
        img.src = 'style-previews/' + id + '.webp';
        sleeve.append(img);
        canvas.append(sleeve);
      }
    }
    motion.pos = motion.target = Math.max(0, styles.indexOf(selected));
    for (const sleeve of canvas.children) sleeve.setAttribute('aria-pressed', String(sleeve.dataset.style === selected));
    wheel = 0;
    measure();
    canvas.focus({ preventScroll: true });
  } };
}
