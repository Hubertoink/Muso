'use strict';

function createPlayerVolume(invoke, heos, toast, onClose) {
  const capsule = document.getElementById('playerVolume');
  const button = document.getElementById('volumeToggle');
  const range = document.getElementById('playerVolumeRange');
  let visit = 0, target = null, pending = null, sending = null;

  function paint(value) {
    range.value = value;
    capsule.style.setProperty('--volume-factor', value / 100);
    range.setAttribute('aria-valuetext', value + '%');
  }
  function close(restoreFocus = false) {
    if (!capsule.classList.contains('open')) return;
    ++visit;
    target = null;
    capsule.classList.remove('open');
    button.setAttribute('aria-expanded', 'false');
    range.disabled = true;
    if (restoreFocus) button.focus({preventScroll:true});
    onClose();
  }
  async function open() {
    const version = ++visit, pid = heos.output;
    capsule.style.setProperty('--volume-factor', 0);
    capsule.classList.add('open');
    button.setAttribute('aria-expanded', 'true');
    try {
      await sending;
      let value, route;
      if (pid == null) {
        value = await invoke('local_volume');
        route = {pid};
        range.setAttribute('aria-label', 'Muso volume');
      } else {
        const snapshot = await heos.readLevels();
        const control = snapshot.controls.find(c => c.group) ?? snapshot.controls.find(c => c.id === pid);
        if (!control) throw new Error('The HEOS output has changed.');
        value = control.value;
        route = {pid, session:snapshot.session, id:control.id, group:control.group, lastValue:value};
        range.setAttribute('aria-label', 'Volume · ' + control.name);
      }
      if (version !== visit || pid !== heos.output) return;
      target = route;
      paint(value);
      range.disabled = false;
      range.focus({preventScroll:true});
    } catch (e) {
      if (version === visit) { close(true); toast(String(e)); }
    }
  }

  // Keep only the newest drag position while a network write is in flight.
  // A release also persists the final local gain.
  async function drain() {
    while (pending) {
      const {route, value, save, version} = pending;
      pending = null;
      try {
        if (route.pid == null) await invoke('local_volume', {value, save});
        else if (route.pid === heos.output && route.lastValue !== value) {
          await heos.writeLevel({session:route.session, id:route.id, group:route.group, value});
          route.lastValue = value;
        }
      } catch (e) {
        if (version === visit) { pending = null; close(true); toast(String(e)); }
      }
      if (!save) await new Promise(resolve => setTimeout(resolve, route.pid == null ? 20 : 100));
    }
  }
  function adjust(save) {
    if (!target) return;
    const value = range.valueAsNumber;
    paint(value);
    pending = {route:target, value, save, version:visit};
    if (!sending) sending = drain().finally(() => { sending = null; });
  }
  range.oninput = () => adjust(false);
  range.onchange = () => adjust(true);
  button.onclick = () => capsule.classList.contains('open') ? close(true) : open();
  capsule.addEventListener('keydown', e => {
    if (e.key === 'Escape') { close(true); e.preventDefault(); }
    e.stopPropagation();
  });
  capsule.addEventListener('focusout', e => {
    if (e.relatedTarget && !capsule.contains(e.relatedTarget)) close();
  });
  document.addEventListener('pointerdown', e => {
    if (!capsule.contains(e.target) && capsule.classList.contains('open')) close();
  }, true);
  return {close, settled:() => sending};
}
