'use strict';

const directionSlop = 10; // CSS pixels
const velocityWindow = 100; // ms
function moveThroughSleeves(gesture, e, current, last) {
  const dx = e.clientX - gesture.x;
  const pos = clamp(current + dx * gesture.indexDirection / gesture.spacing, 0, last);
  gesture.x = e.clientX;

  const samples = gesture.samples;
  while (samples.length && samples[0].t < e.timeStamp - velocityWindow) samples.shift();
  const sample = { pos, t: e.timeStamp };
  const delta = pos - gesture.extreme.pos;
  if (delta && delta * gesture.direction >= 0) {
    gesture.direction = Math.sign(delta);
    gesture.extreme = sample;
  } else if (delta) {
    // Fast swipes can roll back briefly on lift. Count a turn relative to the recent stroke.
    const stroke = samples.length ? Math.abs(gesture.extreme.pos - samples[0].pos) * gesture.spacing : 0;
    if (Math.abs(delta) * gesture.spacing >= clamp(stroke / 3, directionSlop, 32)) {
      samples.length = 0;
      if (gesture.extreme.t >= e.timeStamp - velocityWindow) samples.push(gesture.extreme);
      gesture.direction = Math.sign(delta);
      gesture.extreme = sample;
    }
  }
  samples.push(sample);
  return pos;
}

function settleSwipe(gesture, releasedAt, pos, navigate) {
  const { samples, direction, spacing } = gesture;
  const first = samples.find(sample => sample.t >= releasedAt - velocityWindow) ?? samples.at(-1);
  const distance = pos - first.pos, elapsed = releasedAt - first.t;
  const velocity = elapsed >= 8 && distance * direction * spacing >= directionSlop ? distance / elapsed : 0;
  const flick = Math.abs(velocity) * spacing >= 0.35;
  const touchFlick = gesture.touch && flick;
  const glideTime = touchFlick ? 225 : 170;
  const maxGlide = touchFlick ? 5 : 4;
  const projected = Math.round(pos + clamp(velocity * glideTime, -maxGlide, maxGlide));
  // A quick flick commits even over a short distance. A careful drag can land near a centre.
  const margin = flick ? 0 : Math.min(0.18, 30 / spacing);
  let end = projected;
  if (direction > 0) end = Math.max(end, Math.ceil(pos - margin));
  if (direction < 0) end = Math.min(end, Math.floor(pos + margin));
  navigate(end, velocity ? clamp(Math.abs((end - pos) / velocity), 65, glideTime) : defaultSettleTime);
}


function bindSleeveGestures(surface, motion, { spacing, lastIndex, hold, render, navigate, activate, filing }) {
  let drag = null;

  // WebKitGTK can emit kinetic wheel events after a touch swipe unless we claim touchstart.
  surface.addEventListener('touchstart', e => e.preventDefault(), { passive: false });
  surface.addEventListener('pointerdown', e => {
    if (drag || e.button !== 0 || !e.isPrimary || lastIndex() < 0) return;
    const start = { pos: motion.pos, t: e.timeStamp };
    const touch = e.pointerType === 'touch';
    drag = {
      id: e.pointerId, startX: e.clientX, startY: e.clientY, x: e.clientX, touch,
      indexDirection: touch && swipeDirection.value === 'opposite' ? 1 : -1,
      sleeve: e.target.closest('.sleeve'), mode: 'pending', caught: motion.target !== motion.pos,
      spacing: spacing(), direction: 0, extreme: start, samples: [start],
    };
    motion.target = motion.pos;
    hold();
    surface.setPointerCapture(e.pointerId);
  });

  function track(e) {
    const dx = Math.abs(e.clientX - drag.startX), dy = e.clientY - drag.startY;
    if (drag.mode === 'pending') {
      if (dx > 6 && dx >= Math.abs(dy)) {
        drag.mode = 'browse';
        surface.classList.add('dragging');
      } else if (Math.abs(dy) > 10 && Math.abs(dy) > dx * 1.2) {
        drag.mode = 'vertical';
        if (filing && dy > 0 && drag.sleeve && e.type === 'pointermove') {
          drag.mode = 'file';
          filing.begin(drag.sleeve, e.clientX, e.clientY);
        }
      }
    }
    if (drag.mode === 'file') filing.drag(e.clientX, e.clientY);
    if (drag.mode !== 'browse') return;
    const last = lastIndex();
    for (const sample of e.getCoalescedEvents?.() || []) {
      motion.pos = moveThroughSleeves(drag, sample, motion.pos, last);
    }
    motion.target = motion.pos = moveThroughSleeves(drag, e, motion.pos, last);
    render();
  }
  surface.addEventListener('pointermove', e => {
    if (drag?.id === e.pointerId) track(e);
  });
  function finish(e) {
    if (drag?.id !== e.pointerId || (e.type === 'lostpointercapture' && e.target !== surface)) return;
    const released = e.type === 'pointerup';
    // On touch, the last move is the last sampled position in contact with the screen.
    if (released && !(drag.touch && drag.mode === 'browse')) track(e);
    const gesture = drag;
    drag = null;
    surface.classList.remove('dragging');
    if (gesture.mode === 'file') filing.end(e.clientX, e.clientY, !released);
    if (released && gesture.mode === 'browse') settleSwipe(gesture, e.timeStamp, motion.pos, navigate);
    else if (released && gesture.mode === 'pending' && !gesture.caught && gesture.sleeve &&
        Math.abs(e.clientX - gesture.startX) <= 6 && Math.abs(e.clientY - gesture.startY) <= 10) {
      const i = Number(gesture.sleeve.dataset.index);
      if (i === Math.round(motion.pos)) activate(gesture.sleeve);
      else navigate(i);
    } else navigate(motion.pos);
    render();
  }
  for (const event of ['pointerup', 'pointercancel', 'lostpointercapture']) surface.addEventListener(event, finish);
  return {
    get active() { return drag !== null; },
    reset() {
      const id = drag?.id;
      drag = null;
      surface.classList.remove('dragging');
      if (id != null && surface.hasPointerCapture(id)) surface.releasePointerCapture(id);
    },
  };
}
