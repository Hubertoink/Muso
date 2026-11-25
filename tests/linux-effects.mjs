// Exercise the drawing path in MiniBrowser, the WebKitGTK engine used by Tauri.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';

const server = createServer(async (req, res) => {
  try {
    const path = req.url === '/' ? 'index.html' : req.url.slice(1);
    if (!/^[\w./-]+$/.test(path) || path.includes('..')) throw Error();
    const body = await readFile(new URL('../ui/' + path, import.meta.url));
    const types = { html:'text/html', css:'text/css', js:'text/javascript', png:'image/png', svg:'image/svg+xml' };
    res.setHeader('Content-Type', types[path.split('.').pop()] || 'application/octet-stream');
    res.end(path === 'index.html' ? body.toString().replace('<script src="main.js"></script>', '') : body);
  } catch { res.writeHead(404).end(); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const driver = spawn('WebKitWebDriver', ['--port=4444'], {stdio:'inherit'});
async function command(path, body) {
  const response = await fetch('http://127.0.0.1:4444' + path, body === undefined ? {} : {
    method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body),
  });
  const { value } = await response.json();
  if (!response.ok) throw Error(JSON.stringify(value));
  return value;
}
async function exercise() {
  const byId = id => document.getElementById(id);
  byId('album').className = 'showing-atmos';
  const pictures = byId('atmosPictures'), img = byId('atmosA');
  const art = createLinuxRefraction(pictures);
  img.src = 'wave-field.png';
  await img.decode();
  img.classList.add('on');
  localStorage.setItem('muso.spectrum','1');
  localStorage.setItem('muso.eqStyle','pigment');
  let onBands, draws = 0;
  const spectrum = createSpectrumView(async () => {}, (_, receive) => { onBands = receive; }, {
    clear: art.clear, spectrum(...args) { draws++; art.spectrum(...args); },
  });
  spectrum.setPlayback(true, 1);
  for (let i=0; i<16; i++) onBands({payload:[1, Array(16).fill(.6)]});
  await new Promise(requestAnimationFrame);
  const bounds = pictures.getBoundingClientRect();
  art.pointer(bounds.left + bounds.width/2, bounds.top + bounds.height/2, 200, 1, 2, 25);
  const patches = [...img.parentNode.querySelectorAll('canvas')];
  const dimensions = patches.map(canvas => {
    const pixel = canvas.getContext('2d').getImageData(canvas.width>>1, canvas.height>>1, 1, 1);
    return {w:canvas.width,h:canvas.height,visible:!canvas.hidden,alpha:pixel.data[3]};
  });
  const pngMap = byId('spectrumField').getAttribute('href');
  const trace = byId('eqTrace').getAttribute('d');
  const times = [];
  for (let i = 0; i < 20; i++) {
    await new Promise(requestAnimationFrame);
    const start = performance.now();
    art.pointer(bounds.left+bounds.width/2+i, bounds.top+bounds.height/2, 200, 1, i/3, 25);
    times.push(performance.now()-start);
  }
  spectrum.setPlayback(false, 1);
  art.clear('pointer');
  const pigmentClipped = byId('pigmentBoundary').getAttribute('d') === trace + ' V0 H0 Z';
  return {draws,dimensions,viewHeight:bounds.height,pngMap,trace:!!trace,pigmentClipped,cleared:patches.every(c=>c.hidden),
    pointerSubmitMs:times.sort((a,b)=>a-b)[10]};
}
try {
  for(let i=0;i<50;i++) {
    try {await command('/status'); break;} catch {await delay(100);}
  }
  const {sessionId} = await command('/session',{capabilities:{alwaysMatch:{browserName:'MiniBrowser'}}});
  const session = '/session/'+sessionId;
  await command(session+'/url',{url:'http://127.0.0.1:'+server.address().port+'/'});
  const result = await command(session+'/execute/async',{args:[],script:
    'const done=arguments[arguments.length-1]; ('+ exercise.toString() +')().then(done,e=>done({error:String(e)}));'});
  console.log(result);
  assert.ok(!result.error, result.error);
  assert.equal(result.draws,1);
  assert.ok(result.trace && result.pigmentClipped && result.cleared && !result.pngMap);
  assert.ok(result.dimensions.every(p=>p.visible && p.alpha>0 && p.h<result.viewHeight));
  await fetch('http://127.0.0.1:4444'+session,{method:'DELETE'});
} finally {
  driver.kill();
  server.close();
}
