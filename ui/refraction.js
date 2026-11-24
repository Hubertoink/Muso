'use strict';

// On WebKitGTK a live SVG displacement map can invalidate the whole player.
// Keep the original image and repaint only a patch or a narrow frequency band.
function createLinuxRefraction(pictures) {
  if (!navigator.userAgent.includes('Linux')) return null;
  const layers = [...pictures.querySelectorAll('img')].map(img => {
    const layer = document.createElement('div');
    layer.className = 'painting-layer';
    img.replaceWith(layer);
    layer.append(img);
    const overlays = {};
    for (const name of ['spectrum', 'pointer']) {
      const canvas = document.createElement('canvas');
      canvas.setAttribute('aria-hidden', 'true');
      canvas.hidden = true;
      layer.append(canvas);
      overlays[name] = canvas;
    }
    return { img, layer, overlays };
  });

  function clear(name) {
    for (const { overlays } of layers) overlays[name].hidden = true;
  }

  function patch(name, x, y, width, height) {
    const current = layers.find(({ img }) => img.classList.contains('on') && img.complete && img.naturalWidth);
    if (!current) { clear(name); return; }
    const { img, layer, overlays } = current;
    for (const other of layers) if (other !== current) other.overlays[name].hidden = true;
    const bounds = layer.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    const scale = bounds.width / layer.clientWidth;
    const left = (x - bounds.left) / scale, top = (y - bounds.top) / scale;
    const canvas = overlays[name];
    // Cap raster work on very wide displays; the DOM painting stays full-resolution.
    const resolution = Math.min(1, 1440 / width);
    const w = Math.max(1, Math.round(width * resolution)), h = Math.max(1, Math.round(height * resolution));
    if (canvas.width !== w) canvas.width = w;
    if (canvas.height !== h) canvas.height = h;
    Object.assign(canvas.style, {left:left+'px', top:top+'px', width:width/scale+'px', height:height/scale+'px'});
    const ctx = canvas.getContext('2d');
    const coverScale = Math.max(bounds.width/img.naturalWidth, bounds.height/img.naturalHeight);
    const sx = (img.naturalWidth - bounds.width/coverScale)/2 + (x-bounds.left)/coverScale;
    const sy = (img.naturalHeight - bounds.height/coverScale)/2 + (y-bounds.top)/coverScale;
    ctx.clearRect(0, 0, w, h);
    canvas.hidden = false;
    function sample(dx, dy, dw, dh, sourceX, sourceY, sourceW, sourceH) {
      ctx.drawImage(img, sx + sourceX/coverScale, sy + sourceY/coverScale, sourceW/coverScale, sourceH/coverScale,
        dx*w/width, dy*h/height, dw*w/width, dh*h/height);
    }
    return { ctx, w, h, sample };
  }

  function pointer(x, y, diameter, stretch, phase, force) {
    // Concentric samples deform pixels radially. The outer edge has zero offset.
    const width = Math.ceil(diameter/8)*8, height = Math.ceil(width*stretch/8)*8;
    const p = patch('pointer', x-width/2, y-height/2, width, height);
    if (!p) return;
    for (let ring = 20; ring > 0; ring--) {
      const r = ring / 20;
      const displacement = force * (1-r) * Math.sin(r*14-phase);
      const zoom = 1 + displacement / (diameter*.5);
      p.ctx.save();
      p.ctx.beginPath();
      p.ctx.ellipse(p.w/2, p.h/2, p.w*r/2, p.h*r/2, 0, 0, 2*Math.PI);
      p.ctx.clip();
      p.sample(0, 0, width, height, width*(1-zoom)/2, height*(1-zoom)/2, width*zoom, height*zoom);
      p.ctx.restore();
    }
  }

  function spectrum(centres, amplitudes, gain) {
    const bounds = pictures.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    // Gaussian ripples are negligible beyond this distance from the trace.
    const radius = Math.max(2, Math.ceil(bounds.height*.085));
    const top = Math.floor(Math.min(...centres)*bounds.height)-radius;
    const height = Math.ceil(Math.max(...centres)*bounds.height)+radius-top;
    const p = patch('spectrum', bounds.left, bounds.top+top, bounds.width, height);
    if (!p) return;
    const columns = Math.min(80, Math.ceil(bounds.width/8));
    for (let col = 0; col < columns; col++) {
      const x = Math.round(col*bounds.width/columns);
      const width = Math.round((col+1)*bounds.width/columns)-x;
      const i = Math.round((col+.5)/columns*(centres.length-1));
      const center = centres[i]*bounds.height-top;
      let y = Math.round(center-radius), sourceY = y;
      for (let row = 1; row <= 16; row++) {
        const next = Math.round(center-radius + row*radius/8);
        const d = (next-center)/bounds.height*33;
        const offset = row === 16 ? 0 : 18*gain*amplitudes[i]*Math.exp(-d*d*.55)*Math.sin(d*4);
        const sourceNext = next + offset;
        if (next > y) p.sample(x, y, width, next-y, x, sourceY, width, Math.max(.1,sourceNext-sourceY));
        y = next;
        sourceY = sourceNext;
      }
    }
  }
  return { clear, pointer, spectrum };
}
