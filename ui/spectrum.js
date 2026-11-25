'use strict';

// Several views can want live levels; the backend meter runs while any of them does.
const spectrumDemand = (() => {
  const owners = new Set();
  let sent = false;
  return (invoke, owner, on) => {
    if (on) owners.add(owner); else owners.delete(owner);
    const enabled = owners.size > 0;
    if (enabled === sent) return Promise.resolve();
    sent = enabled;
    return invoke('set_spectrum_enabled', { enabled }).catch(e => { sent = !enabled; owners.delete(owner); throw e; });
  };
})();

function createMiniMeter(invoke, listen, root) {
  const bars = [...root.children];
  const reduce = matchMedia('(prefers-reduced-motion: reduce)');
  const level = new Float32Array(bars.length), target = new Float32Array(bars.length);
  let wanted = false, active = false, trackId, frame = 0, lastEvent = 0;
  function sync() {
    const on = wanted && !reduce.matches && !document.hidden;
    if (on === active) return;
    active = on;
    root.classList.toggle('live', on);
    spectrumDemand(invoke, 'mini', on).catch(() => { active = false; root.classList.remove('live'); });
    if (on) loop(); else { target.fill(0); loop(); }
  }
  function loop() {
    if (frame) return;
    const step = () => {
      frame = 0;
      if (performance.now() - lastEvent > 180) target.fill(0);
      let moving = false;
      for (let i = 0; i < bars.length; i++) {
        level[i] += (target[i] - level[i]) * (target[i] > level[i] ? .5 : .14);
        if (Math.abs(target[i] - level[i]) > .003) moving = true;
        bars[i].style.transform = `scaleY(${(.12 + level[i] * .88).toFixed(3)})`;
      }
      if (active || moving) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
  }
  listen('audio-spectrum', ({ payload: [id, bands] }) => {
    if (!active || id !== trackId) return;
    lastEvent = performance.now();
    const per = bands.length / bars.length;
    for (let i = 0; i < bars.length; i++) {
      let sum = 0;
      for (let k = Math.floor(i * per); k < Math.floor((i + 1) * per); k++) sum = Math.max(sum, bands[k]);
      target[i] = Math.min(1, Math.pow(sum, .7) * 1.25);
    }
  });
  reduce.addEventListener('change', sync);
  document.addEventListener('visibilitychange', sync);
  return {
    set(on, id) {
      wanted = on;
      trackId = id;
      sync();
    },
  };
}

// Join successive midpoints with quadratic curves so the rim has a continuous tangent.
function pigmentTrace(centres) {
  const step = 1000 / (centres.length - 1);
  let trace = 'M0 ' + (centres[0] * 100).toFixed(3);
  for (let i = 1; i < centres.length - 1; i++) {
    trace += ' Q' + (i * step).toFixed(2) + ' ' + (centres[i] * 100).toFixed(3)
      + ' ' + ((i + .5) * step).toFixed(2) + ' ' + ((centres[i] + centres[i + 1]) * 50).toFixed(3);
  }
  return trace + ' L1000 ' + (centres[centres.length - 1] * 100).toFixed(3);
}

function createSpectrumView(invoke, listen, localRefraction, settingsChanged) {
  const control = id => document.getElementById(id);
  const toggle = control('spectrumToggle'), style = control('eqStyle'), colour = control('eqColour');
  const strength = control('eqIntensity'), height = control('eqHeight'), sensitivity = control('eqSensitivity');
  const view = createSpectrumRenderer(invoke, listen, localRefraction);
  function settings() {
    return {enabled:toggle.checked, style:style.value, colour:colour.value, intensity:Number(strength.value),
      height:Number(height.value), sensitivity:Number(sensitivity.value)};
  }
  function apply() {
    control('eqColourRow').classList.toggle('hidden', style.value !== 'pigment');
    view.setSettings(settings());
  }
  toggle.checked = localStorage.getItem('muso.spectrum') === '1';
  toggle.addEventListener('change', () => {
    localStorage.setItem('muso.spectrum', toggle.checked ? '1' : '0');
    apply();
    settingsChanged?.();
  });
  for (const input of [style, colour, strength, height, sensitivity]) {
    const key = 'muso.' + input.id;
    const saved = localStorage.getItem(key);
    if (saved !== null) input.value = saved;
    if (input === style && !style.value) style.value = 'ripple';
    const output = document.querySelector('output[for="' + input.id + '"]');
    const update = () => { if (output) output.value = input.value + '%'; };
    update();
    input.addEventListener(input === style ? 'change' : 'input', () => {
      localStorage.setItem(key, input.value);
      update();
      apply();
      settingsChanged?.();
    });
  }
  apply();
  return {settings, setPlayback:view.setPlayback};
}

function createSpectrumRenderer(invoke, listen, localRefraction) {
  const control = id => document.getElementById(id);
  let settings = {enabled:false};
  const pigment = control('spectrumPigment');
  const boundary = control('pigmentBoundary');
  const brush = pigment.getContext('2d');
  const surface = document.getElementById('spectrumSurface');
  const line = document.getElementById('spectrumLine');
  const path = line.querySelector('path');
  const field = document.getElementById('spectrumField');
  const depth = document.getElementById('spectrumDepth');
  const reduce = matchMedia('(prefers-reduced-motion: reduce)');
  const levels = new Float32Array(16);
  // The displacement map, pigment and line share the full artwork's coordinates.
  const canvas = document.createElement('canvas');
  canvas.width = 192;
  canvas.height = 128;
  const ctx = canvas.getContext('2d');
  const map = ctx.createImageData(canvas.width, canvas.height);
  map.data.fill(255);
  let visible = false, active = false, trackId;
  let drawFrame = 0, latest;
  const centres = new Float32Array(canvas.width), amplitudes = new Float32Array(canvas.width);
  function clear() {
    cancelAnimationFrame(drawFrame);
    drawFrame = 0;
    latest = null;
    localRefraction?.clear('spectrum');
    surface.classList.remove('active');
    line.style.opacity = '0';
    pigment.style.opacity = '0';
    levels.fill(0);
  }
  function sync() {
    const enabled = visible && settings.enabled && !reduce.matches && !document.hidden && settings.intensity > 0;
    if (enabled === active) return;
    active = enabled;
    if (!enabled) clear();
    spectrumDemand(invoke, 'atmosphere', enabled).catch(() => { active = false; clear(); });
  }
  reduce.addEventListener('change', sync);
  document.addEventListener('visibilitychange', sync);

  function draw(bands) {
    const sensitivityGain = settings.sensitivity / 100;
    for (let i = 0; i < levels.length; i++) {
      const target = Math.min(1, bands[i] * sensitivityGain);
      levels[i] += (target - levels[i]) * (target > levels[i] ? .64 : .23);
    }
    const peak = Math.max(...levels);
    if (peak < .015) { clear(); return; }
    const gain = settings.intensity / 100;
    const reach = settings.height / 100;
    const painting = settings.style === 'pigment';
    if (painting) brush.clearRect(0, 0, pigment.width, pigment.height);
    const points = [];
    const w = canvas.width, h = canvas.height;
    for (let x = 0; x < w; x++) {
      const fraction = x / (w - 1);
      const index = fraction * (levels.length - 1);
      const band = Math.floor(index);
      const t = index - band;
      const amplitude = levels[band] + (levels[Math.min(band + 1, levels.length - 1)] - levels[band]) * t * t * (3 - 2 * t);
      const edge = Math.min(1, fraction * 14, (1 - fraction) * 14);
      const taper = painting ? edge * edge * (3 - 2 * edge) : edge;
      const envelope = amplitude * taper;
      const center = .774 - envelope * reach * .22;
      if (painting) {
        const base = center * pigment.height;
        const lift = Math.max(1, envelope * reach * .19 * pigment.height);
        const wash = brush.createLinearGradient(0, base, 0, base - lift);
        wash.addColorStop(0, settings.colour);
        wash.addColorStop(.35, settings.colour + 'cf');
        wash.addColorStop(1, settings.colour + '00');
        brush.fillStyle = wash;
        // Extend the wash under the trace. The shared vector curve cuts its visible edge
        // at display resolution, including between neighbouring colour samples.
        brush.fillRect(x, base - lift, 1, pigment.height - base + lift);
      }
      centres[x] = center;
      if (!painting && (x % 2 === 0 || x === w - 1)) points.push((fraction * 1000).toFixed(1) + ' ' + (center * 100).toFixed(1));
      if (localRefraction) {
        amplitudes[x] = envelope;
        continue;
      }
      for (let y = 0; y < h; y++) {
        const dist = (y / (h - 1) - center) * 33;
        const ripple = envelope * Math.exp(-dist * dist * .55) * Math.sin(dist * 4);
        const offset = 4 * (y * w + x);
        map.data[offset] = 128;
        map.data[offset + 1] = 128 + 124 * ripple;
      }
    }
    if (localRefraction) {
      localRefraction.spectrum(centres, amplitudes, gain);
    } else {
      ctx.putImageData(map, 0, 0);
      field.setAttribute('href', canvas.toDataURL('image/png'));
      depth.setAttribute('scale', 38 * gain);
      surface.classList.add('active');
    }
    const trace = painting ? pigmentTrace(centres) : 'M' + points.join(' L');
    path.setAttribute('d', trace);
    if (painting) boundary.setAttribute('d', trace + ' V0 H0 Z');
    line.style.opacity = String(gain * Math.min(1, 1.65 * peak));
    if (painting) pigment.style.opacity = String(gain * Math.min(1, 2 * peak));
  }

  listen('audio-spectrum', ({ payload: [id, bands] }) => {
    if (!active || id !== trackId || bands.length !== levels.length) return;
    latest = bands;
    if (!drawFrame) drawFrame = requestAnimationFrame(() => {
      drawFrame = 0;
      draw(latest);
    });
  });
  return {
    setSettings(next) {
      if (settings.style !== next.style) {
        control('eqEchoes').style.display = next.style === 'echo' ? '' : 'none';
        clear();
      }
      settings = next;
      sync();
    },
    setPlayback(display, id) {
      if (id !== trackId) clear();
      visible = display;
      trackId = id;
      sync();
    },
  };
}
