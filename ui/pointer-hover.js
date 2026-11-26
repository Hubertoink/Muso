'use strict';

// On hybrid devices a tap can leave :hover matched until the mouse moves again.
const rootClasses = document.documentElement.classList;
rootClasses.toggle('can-hover', matchMedia('(any-hover: hover)').matches);
// Only the current mouse target gets a native title. Touch-generated mouse
// compatibility events can otherwise keep the WebView's tooltip on screen.
let tooltipTarget = null;
function syncTooltipTitle() {
  const description = tooltipTarget?.getAttribute('aria-description');
  if (description) tooltipTarget.title = description;
  else tooltipTarget?.removeAttribute('title');
}
const tooltipObserver = new MutationObserver(syncTooltipTitle);
function setTooltipTarget(target) {
  if (target === tooltipTarget) return;
  tooltipObserver.disconnect();
  tooltipTarget?.removeAttribute('title');
  tooltipTarget = target;
  if (target) {
    syncTooltipTitle();
    tooltipObserver.observe(target, { attributes: true, attributeFilter: ['aria-description'] });
  }
}
// WebKitGTK can report a compatibility event as an ordinary mouse event.
// A parked cursor and the contact point can both receive it after a tap.
let lastMouse = null, lastContact = null, touchCount = 0;
const contacts = new Set();
function atPosition(point, e) {
  return point && Math.abs(point.x - e.screenX) <= 1 && Math.abs(point.y - e.screenY) <= 1;
}
function suspendHover(contact) {
  if (contact) lastContact = {x:contact.screenX, y:contact.screenY};
  rootClasses.remove('can-hover');
  setTooltipTarget(null);
}
function updatePointerHover(e) {
  if (e.pointerType !== 'mouse') {
    if (e.type === 'pointerdown') contacts.add(e.pointerId);
    if (e.type === 'pointerup' || e.type === 'pointercancel') contacts.delete(e.pointerId);
    suspendHover(e);
    return;
  }
  if (e.sourceCapabilities?.firesTouchEvents) { suspendHover(e); return; }
  if (touchCount || contacts.size || e.type === 'pointerup' || e.type === 'pointercancel') return;
  if (!rootClasses.contains('can-hover') &&
      (e.type !== 'pointermove' || atPosition(lastMouse, e) || atPosition(lastContact, e))) return;
  lastMouse = {x:e.screenX, y:e.screenY};
  rootClasses.add('can-hover');
  setTooltipTarget(e.target.closest?.('[aria-description]') ?? null);
}
for (const event of ['pointerover', 'pointerdown', 'pointermove', 'pointerup', 'pointercancel']) {
  window.addEventListener(event, updatePointerHover, {capture:true, passive:true});
}
// Some WebViews dispatch Touch Events before emulating the mouse pointer.
for (const event of ['touchstart', 'touchend', 'touchcancel']) {
  window.addEventListener(event, e => {
    touchCount = e.touches.length;
    suspendHover(e.changedTouches[0]);
  }, {capture:true, passive:true});
}
document.addEventListener('pointerout', e => {
  if (!tooltipTarget?.contains(e.relatedTarget)) setTooltipTarget(null);
}, true);
window.addEventListener('blur', () => {
  contacts.clear();
  touchCount = 0;
  suspendHover();
});
