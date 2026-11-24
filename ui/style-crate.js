'use strict';

function createStyleCrate(dialog, names, choose) {
  const canvas = $('styleCanvas');
  const styles = Object.keys(names);
  const last = styles.length - 1;
  let position = 0, target = 0, size = 0;
  let frame = 0, gesture = null, dragged = false, wheel = 0;

  function paint() {
    for (let i = 0; i < styles.length; i++) {
      const sleeve = canvas.children[i];
      sleeve.hidden = Math.abs(i - position) >= WINDOW;
      if (!sleeve.hidden) positionSleeve(sleeve, i - position, size);
    }
    const index = Math.round(target);
    const label = names[styles[index]];
    if ($('styleCaption').textContent !== label) $('styleCaption').textContent = label;
    $('prevStyle').disabled = index === 0;
    $('nextStyle').disabled = index === last;
  }

  function navigate(index, time = defaultSettleTime) {
    cancelAnimationFrame(frame);
    target = clamp(Math.round(index), 0, last);
    let stamp = performance.now();
    function tick(now) {
      const diff = target - position;
      position = reducedMotion.matches || Math.abs(diff) * size < 0.25
        ? target : position + diff * -Math.expm1(-(now - stamp) / time);
      paint();
      stamp = now;
      if (position !== target) frame = requestAnimationFrame(tick);
    }
    tick(stamp);
  }

  $('prevStyle').onclick = () => navigate(target - 1);
  $('nextStyle').onclick = () => navigate(target + 1);
  $('useStyle').onclick = () => choose(styles[Math.round(target)]);
  dialog.addEventListener('keydown', e => {
    if (e.key === 'ArrowLeft') navigate(target - 1);
    else if (e.key === 'ArrowRight') navigate(target + 1);
    else if (e.key === 'Home') navigate(0);
    else if (e.key === 'End') navigate(last);
    else if (e.target === canvas && ['Enter', ' '].includes(e.key)) choose(styles[Math.round(target)]);
    else return;
    e.preventDefault();
  });
  canvas.addEventListener('wheel', e => {
    e.preventDefault();
    if (gesture) return;
    const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    wheel += delta * (e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? canvas.clientWidth : 1);
    const steps = Math.trunc(wheel / 70);
    if (steps) { wheel -= steps * 70; navigate(target + steps); }
  }, { passive: false });

  canvas.addEventListener('pointerdown', e => {
    if (!e.isPrimary || e.button !== 0 || gesture) return;
    const start = { pos: position, t: e.timeStamp };
    const touch = e.pointerType === 'touch';
    gesture = { id: e.pointerId, x: e.clientX, startX: e.clientX, touch, spacing: size * 0.66,
      indexDirection: touch && swipeDirection.value === 'opposite' ? 1 : -1,
      samples: [start], extreme: start, direction: 0 };
    dragged = false;
    wheel = 0;
  });
  canvas.addEventListener('pointermove', e => {
    if (gesture?.id !== e.pointerId) return;
    if (!dragged) {
      if (Math.abs(e.clientX - gesture.startX) <= 6) return;
      dragged = true;
      cancelAnimationFrame(frame);
      canvas.setPointerCapture(e.pointerId);
    }
    position = target = moveThroughSleeves(gesture, e, position, last);
    paint();
  });
  function finish(e) {
    if (gesture?.id !== e.pointerId || (e.type === 'lostpointercapture' && e.target !== canvas)) return;
    const ended = gesture;
    gesture = null;
    if (!dragged) return;
    if (e.type === 'pointerup') settleSwipe(ended, e.timeStamp, position, navigate);
    else navigate(position);
  }
  for (const event of ['pointerup', 'pointercancel', 'lostpointercapture']) canvas.addEventListener(event, finish);
  canvas.addEventListener('pointerleave', e => {
    if (!dragged && gesture?.id === e.pointerId) gesture = null;
  });
  canvas.addEventListener('click', e => {
    const sleeve = e.target.closest('[data-style]');
    if (!sleeve || (dragged && e.detail)) return;
    const index = styles.indexOf(sleeve.dataset.style);
    if (index === Math.round(position) && index === Math.round(target)) choose(styles[index]);
    else {
      canvas.focus({ preventScroll: true });
      navigate(index);
    }
  });

  function measure() {
    if (!dialog.open || !canvas.firstElementChild) return;
    size = canvas.firstElementChild.offsetWidth || canvas.querySelector(':not([hidden])').offsetWidth;
    paint();
  }
  new ResizeObserver(measure).observe(canvas);
  dialog.addEventListener('close', () => {
    cancelAnimationFrame(frame);
    gesture = null;
  });
  return { open(selected) {
    if (!canvas.childElementCount) {
      for (const id of styles) {
        const sleeve = document.createElement('button');
        sleeve.type = 'button';
        sleeve.className = 'sleeve';
        sleeve.tabIndex = -1;
        sleeve.dataset.style = id;
        sleeve.setAttribute('aria-label', names[id]);
        const img = new Image();
        img.alt = '';
        img.draggable = false;
        img.src = 'style-previews/' + id + '.webp';
        sleeve.append(img);
        canvas.append(sleeve);
      }
    }
    position = target = Math.max(0, styles.indexOf(selected));
    for (const sleeve of canvas.children) sleeve.setAttribute('aria-pressed', String(sleeve.dataset.style === selected));
    wheel = 0;
    dragged = false;
    measure();
    canvas.focus({ preventScroll: true });
  } };
}
