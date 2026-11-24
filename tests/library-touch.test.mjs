import assert from 'node:assert/strict';
import { after, before, beforeEach, afterEach, test } from 'node:test';
import { chromium } from 'playwright-core';

let browser, page, input, now;
before(async () => {
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
});
after(() => browser?.close());
beforeEach(async () => {
  page = await browser.newPage({ viewport: { width: 1200, height: 800 }, hasTouch: true });
  await page.addInitScript(() => {
    const albums = Array.from({ length: 80 }, (_, i) => ({
      id: i.toString(16).padStart(6, '0'), artist: 'Test', title: 'Record ' + i,
    }));
    localStorage.setItem('muso.libraryPosition', JSON.stringify({ index: 20, id: albums[20].id }));
    const events = new Map();
    window.emitPlayback = (name, payload) => events.get(name)({ payload });
    window.dateAlbum = (index, timestamp) => { albums[index].first_seen = timestamp; };
    window.refreshLibrary = () => {
      albums[19].title = 'Updated record';
      return events.get('scan-done')({ payload: {} });
    };
    const crates = { active: null, lists: [] };
    window.fillCrate = count => {
      crates.lists = [{ id: 1, name: 'Records', albums: albums.slice(0, count).map(({ id }) => ({ id })) }];
    };
    window.__TAURI__ = {
      core: { invoke: async (command, args) => {
        if (command === 'get_folders') { await window.folderGate; return ['test']; }
        if (command === 'get_albums') return structuredClone(albums);
        if (command === 'upgrade_cover') {
          window.coverRequest = args;
          return window.coverResult;
        }
        if (command === 'genre_state') return {genres:[{name:'Electronic', parent:null}], albums:{}, suggestions:0};
        if (command === 'genre_inventory') return {tags:[], broad:[], records:albums.length, untagged:albums.length};
        if (command === 'genre_classify') {
          window.sortRequest = args;
          return window.sortResult ?? {suggestions:0, uncertain:0, attempted:0};
        }
        if (command === 'get_setting') return sessionStorage.getItem(args.key);
        if (command === 'set_setting') return sessionStorage.setItem(args.key, args.value.trim());
        if (command === 'has_environment_key') return sessionStorage.getItem('test.envKey') === 'yes';
        if (command === 'has_openai_key') return Boolean(sessionStorage.getItem('openai_key')) || sessionStorage.getItem('test.envKey') === 'yes';
        if (command === 'get_album_note') return sessionStorage.getItem('note.' + args.albumId);
        if (command === 'generate_album_note') {
          sessionStorage.setItem('note.' + args.albumId, 'Saved album article');
          return 'Saved album article';
        }
        if (command === 'delete_album_note') return sessionStorage.removeItem('note.' + args.albumId);
        if (command === 'library_crates') {
          if (args.change?.type === 'add') {
            const list = crates.lists.find(c => c.id === args.change.id);
            if (list) list.albums.push({id: args.change.album});
            else crates.lists.push({ id: crates.lists.length + 1, name: 'Filed records', albums: [{ id: args.change.album }] });
          }
          return structuredClone(crates);
        }
        if (command === 'get_tracks') return window.testTracks ?? [{ path: 'test', title: 'Track' }];
        if (command === 'get_atmospheres') return window.testPaintings ?? [];
        if (command === 'generate_atmospheres') {
          (window.generationRequests ??= []).push(args);
          return [];
        }
      } },
      event: { listen: async (name, fn) => events.set(name, fn) },
      app: { getVersion: async () => 'test' },
      window: { getCurrentWindow: () => ({ isFullscreen: async () => false }) },
    };
  });
  await page.goto(new URL('../ui/index.html', import.meta.url).href);
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  input = await page.context().newCDPSession(page);
  now = Date.now() / 1000;
});
afterEach(() => page?.close());

// Send touch through the browser's input pipeline with a repeatable finger speed.
async function touch(type, x, elapsed = 16, y = 350) {
  now += elapsed / 1000;
  await input.send('Input.dispatchTouchEvent', {
    type, timestamp: now, touchPoints: x == null ? [] : [{ x, y, id: 1 }],
  });
}
const down = () => touch('touchStart', 600);
const move = (x, ms, y) => touch('touchMove', x, ms, y);
const up = ms => touch('touchEnd', null, ms);
async function position() {
  return page.evaluate(() => state.pos);
}
async function settled() {
  await page.waitForFunction(() => state.pos === state.target && Number.isInteger(state.pos));
  return position();
}

test('cover lookup distinguishes missing artwork and reserves AI offers for an individual search', async () => {
  await page.evaluate(async () => {
    window.coverResult = {status: 'no_front_cover'};
    await findCovers([focused()], true);
  });
  assert.equal(await page.locator('#toast').textContent(), 'Release found, but no front cover was available');
  assert.equal(await page.evaluate(() => window.coverRequest.offerAi), true);
  await page.evaluate(async () => {
    window.coverResult = {status: 'no_match'};
    await findCovers([focused()]);
  });
  assert.equal(await page.evaluate(() => window.coverRequest.offerAi), false);
});

test('genre sorting remembers custom instructions, including an empty preference', async () => {
  const field = page.getByLabel('Genre sorting instructions', {exact:false});
  await page.evaluate(() => openGenres({tab:'sort'}));
  assert.match(await field.inputValue(), /Ambient/);
  const rule = 'Ambient als eigenes Genre vorschlagen.';
  await field.fill(rule);
  await page.reload();
  await page.evaluate(() => openGenres({tab:'sort'}));
  assert.equal(await field.inputValue(), rule);
  await page.locator('#sortResearch').check();
  await page.locator('#startSort').click();
  const request = await page.evaluate(() => window.sortRequest);
  assert.equal(request.instructions, rule);
  assert.equal(request.research, true);
  await field.fill('');
  await page.reload();
  assert.equal(await field.inputValue(), '');
});

test('genre research shows the uncertain count and uses the lookup total for progress', async () => {
  await page.evaluate(() => {
    window.sortResult = new Promise(resolve => { window.finishSort = resolve; });
    return openGenres({tab:'sort'});
  });
  await page.locator('#sortResearch').check();
  await page.locator('#startSort').click();
  await page.evaluate(() => window.emitPlayback('genre-progress', {stage:'Sorting', done:75, total:80, uncertain:51}));
  assert.match(await page.locator('#sortProgressText').textContent(), /51 uncertain so far/);
  await page.evaluate(() => window.emitPlayback('genre-progress', {stage:'Researching', done:1, total:52, uncertain:52}));
  assert.equal(await page.locator('#sortProgressText').textContent(), 'Looking up uncertain album 2 of 52');
  await page.evaluate(() => window.finishSort({suggestions:0, uncertain:52, attempted:2}));
  await page.waitForFunction(() => document.getElementById('genreStatus').textContent.includes('2 of 52'));
});

test('NEU shows recent releases newest first, restores the bookmark and expires automatically', async () => {
  const button = page.locator('#newAlbums');
  assert.ok(await button.isDisabled());
  await page.clock.install();
  await page.evaluate(async () => {
    const cutoff = Math.floor(Date.now() / 1000) - 28 * 86400;
    dateAlbum(3, cutoff + 60);
    dateAlbum(5, cutoff + 90);
    dateAlbum(7, cutoff);
    await loadAlbums();
  });
  await button.click();
  assert.deepEqual(await page.evaluate(() => state.albums.map(a => a.title)), ['Record 5', 'Record 3']);
  await button.click();
  assert.equal(await page.evaluate(() => focused().title), 'Record 20');
  await button.click();
  await page.locator('#indexLetters button:enabled').first().click();
  assert.equal(await button.getAttribute('aria-pressed'), 'false');
  assert.equal(await page.evaluate(() => state.albums.length), 80);
  await button.click();
  await page.clock.fastForward(100000);
  assert.ok(await button.isDisabled());
  assert.equal(await page.evaluate(() => state.albums.length), 80);
});

test('a left swipe moves the sleeve right and a library refresh preserves its position', async () => {
  await down();
  await move(540);
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  const offset = await page.locator('.sleeve[data-index="20"]').evaluate(el =>
    new DOMMatrix(getComputedStyle(el).transform).m41);
  assert.ok(Math.abs(offset - 60) < 2, 'a 60 px left swipe moved the sleeve by ' + offset);
  const held = await position();
  await page.evaluate(() => { window.refreshDone = window.refreshLibrary(); });
  assert.equal(await position(), held);
  await up(220);
  assert.equal(await settled(), 19);
  await page.evaluate(() => window.refreshDone);
  assert.equal(await page.locator('#capTitle').textContent(), 'Updated record');
});

for (const sign of [-1, 1]) {
  test('small lift jitter keeps the committed swipe direction ' + sign, async () => {
    await down();
    await move(600 + sign * 30);
    await move(600 + sign * 74);
    // A pause followed by 5 px of fingertip roll in the opposite direction.
    await move(600 + sign * 73, 200);
    await move(600 + sign * 69, 16);
    await up(1);
    assert.equal(await settled(), 20 + sign);
  });
}

test('a cancelled flick settles on the sleeve under the finger', async () => {
  await down();
  await move(555);
  await move(510);
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  const nearest = await page.evaluate(() => Math.round(state.target));
  await touch('touchCancel', null);
  assert.equal(await settled(), nearest);
  assert.equal(await page.locator('#album').isVisible(), false);
});

test('a fast flick glides, and another touch catches it without opening an album', async () => {
  await down();
  await move(530);
  await move(460);
  await up();
  const end = await page.evaluate(() => state.target);
  assert.ok(end >= 15 && end <= 18, 'unexpected flick destination ' + end);
  await down();
  const caught = await position();
  await page.waitForTimeout(120);
  assert.equal(await position(), caught);
  await up();
  assert.equal(await settled(), Math.round(caught));
  assert.equal(await page.locator('#album').isVisible(), false);
});

test('a deliberate reversal before lifting changes the landing sleeve', async () => {
  await down();
  await move(365);
  await move(390);
  await move(520);
  await up(220);
  assert.equal(await settled(), 20);
});

test('changing direction at the collection edge moves immediately', async () => {
  await page.keyboard.press('Home');
  assert.equal(await settled(), 0);
  await down();
  await move(470);
  await move(550);
  await up(220);
  assert.equal(await settled(), 1);
  await page.keyboard.press('End');
  assert.equal(await settled(), 79);
  await down();
  await move(800);
  await up();
  assert.equal(await settled(), 79);
});

test('a downward pull opens the crate tray and a tap opens the centred record', async () => {
  await down();
  await move(604, 32, 420);
  await page.locator('#crateTray.open').waitFor();
  await up();
  await page.locator('#crateTray.open').waitFor({ state: 'detached' });
  assert.equal(await position(), 20);
  await down();
  await up();
  await page.locator('#album').waitFor();
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 20');
});

test('a short quick swipe to the left centres the previous sleeve', async () => {
  await down();
  await move(581, 16);
  await up(8);
  assert.equal(await settled(), 19);
});

test('losing pointer capture cancels the glide and permits the next gesture', async () => {
  await page.evaluate(() => {
    crate.addEventListener('gotpointercapture', e => { window.captured = e.pointerId; }, { once: true });
  });
  await down();
  await move(530);
  await page.waitForFunction(() => window.captured !== undefined);
  await page.evaluate(() => crate.releasePointerCapture(window.captured));
  await move(480);
  await up();
  assert.equal(await settled(), 20);
  await down();
  await move(530);
  await up(220);
  assert.equal(await settled(), 19);
});

test('dragging left with a mouse moves the sleeve left', async () => {
  await page.mouse.move(600, 350);
  await page.mouse.down();
  await page.mouse.move(540, 350);
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)));
  const offset = await page.locator('.sleeve[data-index="20"]').evaluate(el =>
    new DOMMatrix(getComputedStyle(el).transform).m41);
  assert.ok(Math.abs(offset + 60) < 2, 'a 60 px mouse drag to the left moved the sleeve by ' + offset);
  await page.mouse.up();
});

test('a touch tap clears mouse hover while keyboard focus and the next mouse move still highlight the index', async () => {
  const mode = page.locator('#indexMode');
  const color = () => mode.evaluate(el => getComputedStyle(el).color);
  const rest = await color();
  await mode.hover();
  const highlighted = await color();
  assert.notEqual(highlighted, rest);
  await mode.tap();
  assert.equal(await page.locator('#indexModeLabel').textContent(), 'Album');
  assert.equal(await color(), rest);
  const current = page.locator('#indexLetters [aria-current=true]');
  const scale = () => current.evaluate(el => Number(getComputedStyle(el).getPropertyValue('--letter-scale')));
  assert.equal(await scale(), 1);
  await page.keyboard.press('Tab');
  assert.ok(await current.evaluate(el => el.matches(':focus-visible')));
  assert.equal(await scale(), 1.8);
  await mode.hover({ position: { x: 5, y: 5 } });
  assert.equal(await color(), highlighted);
});

async function setApiKey(dialog, value) {
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=' + dialog + ']').click();
  await page.locator('#' + dialog + ' input[type=password]').fill(value);
  await page.locator('#' + dialog + ' .close').click();
}

test('the write action follows the saved key and existing articles remain readable and deletable without it', async () => {
  const write = page.getByRole('button', { name: 'Write album article with AI' });
  await page.waitForFunction(() => visibleNote?.album);
  assert.equal(await page.locator('#notePreview').isVisible(), false);
  await setApiKey('settings', 'fixture-key');
  await write.waitFor();
  await page.reload();
  await write.click();
  const read = page.getByRole('button', { name: 'Read album article' });
  await read.waitFor();
  await setApiKey('ai', '   ');
  await read.click();
  assert.equal(await page.locator('#articleBody').textContent(), 'Saved album article');
  await page.locator('#articleMore').click();
  assert.equal(await page.locator('#rewriteArticle').isVisible(), false);
  const remove = page.locator('#deleteArticle');
  assert.ok(await remove.evaluate(el => el === document.activeElement));
  await remove.click();
  await remove.click();
  await page.locator('#notePreview').waitFor({ state: 'hidden' });
  assert.equal(await write.count(), 0);
});

test('an environment key enables the write action at startup and after the saved key is removed', async () => {
  await page.evaluate(() => sessionStorage.setItem('test.envKey', 'yes'));
  await page.reload();
  const write = page.getByRole('button', { name: 'Write album article with AI' });
  await write.waitFor();
  await setApiKey('ai', 'fixture-key');
  await setApiKey('settings', '');
  assert.equal(await write.isVisible(), true);
});

test('an article saved during painting becomes readable on the focused sleeve', async () => {
  await page.waitForFunction(() => visibleNote?.album);
  await page.evaluate(() => {
    const id = focused().id;
    sessionStorage.setItem('note.' + id, 'From the shared research');
    window.emitPlayback('article-saved', id);
  });
  await page.getByRole('button', { name: 'Read album article' }).click();
  assert.equal(await page.locator('#articleBody').textContent(), 'From the shared research');
});

test('the touch preference works immediately and survives a reload', async () => {
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=settings]').click();
  const select = page.getByLabel('Library swipe direction');
  assert.equal(await select.inputValue(), 'opposite');
  await select.selectOption('follow');
  await page.locator('#settings .close').click();
  await down();
  await move(540);
  await up(220);
  assert.equal(await settled(), 21);

  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await down();
  await move(660);
  await up(220);
  assert.equal(await settled(), 19);
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=settings]').click();
  assert.equal(await select.inputValue(), 'follow');
  await select.selectOption('opposite');
  await page.locator('#settings .close').click();
  await down();
  await move(540);
  await up(220);
  assert.equal(await settled(), 18);
});

test('the menu keeps its backdrop while Settings loads and the scrollbar clears both corners', async () => {
  await page.evaluate(() => { window.folderGate = new Promise(resolve => { window.readyFolders = resolve; }); });
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=settings]').click();
  assert.equal(await page.locator('#menu').evaluate(el => el.inert), false);
  await page.evaluate(() => window.readyFolders());
  const scroll = page.locator('#settings .modal-scroll');
  await scroll.waitFor();
  const insideCorners = await scroll.evaluate(el => {
    const card = el.parentElement, shell = card.getBoundingClientRect(), viewport = el.getBoundingClientRect();
    const radius = parseFloat(getComputedStyle(card).borderRadius);
    return viewport.top - shell.top >= radius && shell.bottom - viewport.bottom >= radius;
  });
  assert.ok(insideCorners);
  await page.locator('#settings .close').scrollIntoViewIfNeeded();
  assert.ok(await scroll.evaluate(el => el.scrollTop > 0));
  await page.locator('#settings .close').click();
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=eq]').click();
  await page.locator('#eq').waitFor();
  assert.equal(await page.locator('#spectrumToggle').evaluate(el => el === document.activeElement), true);
});

for (const device of ['touch', 'mouse']) {
  test(device + ' can file the playing record in the left tray without changing playback', async () => {
    if (device === 'mouse') await page.evaluate(async () => { window.fillCrate(1); await albumCrates.load(); });
    await down(); await up();
    await page.locator('#album:not(.hidden) #cover').waitFor();
    const playing = () => page.evaluate(() => [state.view, state.playing.album.id, state.playing.playbackId, state.playing.paused]);
    const before = await playing();
    if (device === 'touch') { await down(); await move(520, 32); }
    else { await page.mouse.move(600, 350); await page.mouse.down(); await page.mouse.move(520, 350); }
    const targetY = device === 'touch' ? 400 : 330;
    await page.waitForFunction(({ y, id }) => document.elementFromPoint(80, y)?.closest('.tray-crate')?.dataset.id === id,
      { y: targetY, id: device === 'touch' ? '' : '1' });
    const point = x => device === 'touch' ? move(x, 32, targetY) : page.mouse.move(x, targetY);
    await point(265);
    await page.waitForFunction(() => {
      const opacity = +getComputedStyle(document.querySelector('.crate-ghost')).opacity;
      return opacity > .4 && opacity < .99;
    });
    await point(80);
    await page.waitForFunction(() => {
      const style = getComputedStyle(document.querySelector('.crate-ghost'));
      return +style.opacity < .4 && parseFloat(style.filter.slice(5)) > 5;
    });
    await point(520);
    await page.waitForFunction(() => +getComputedStyle(document.querySelector('.crate-ghost')).opacity > .99);
    await point(80);
    if (device === 'touch') await up(); else await page.mouse.up();
    await page.locator('#crateTray.side .tray-crate.landed').waitFor();
    const crates = await page.evaluate(() => invoke('library_crates', {}));
    assert.equal(crates.lists.length, 1);
    assert.equal(crates.lists[0].albums.at(-1).id, before[1]);
    assert.deepEqual(await playing(), before);
  });
}

test('an interrupted return cannot brighten the source of the next pull', async () => {
  await down();
  await move(600, 32, 450);
  await up();
  await down();
  await move(600, 32, 465);
  await page.locator('#crateTray.open').waitFor();
  // Hold longer than the interrupted return flight.
  await page.waitForTimeout(650);
  assert.equal(await page.locator('.crate-ghost').count(), 1);
  assert.equal(await page.locator('.sleeve.filing').getAttribute('data-index'), '20');
  await touch('touchCancel', null);
  await page.locator('.crate-ghost').waitFor({ state: 'detached' });
  assert.equal(await page.locator('.sleeve.filing').count(), 0);
});

test('filing keeps the source dim until the copy lands, including through a scan', async () => {
  await down();
  await move(600, 32, 460);
  // Wait until the tray has reached the drop point before releasing.
  await page.waitForFunction(() => document.elementFromPoint(600, 730)?.closest('.tray-crate.new'));
  await move(600, 32, 730);
  await up();
  const copy = page.locator('.crate-ghost');
  await copy.evaluate(el => el.getAnimations().find(a => !(a instanceof CSSTransition)).pause());
  await page.evaluate(() => window.refreshLibrary());
  assert.equal(await page.locator('.sleeve.filing').getAttribute('data-index'), '20');
  await copy.evaluate(el => el.getAnimations().find(a => !(a instanceof CSSTransition)).finish());
  await page.locator('.tray-crate.landed').waitFor();
  assert.equal(await page.locator('.sleeve.filing').count(), 0);
  assert.equal(await copy.count(), 0);
});

for (const preference of ['follow', 'opposite']) {
  for (const fingerDirection of [-1, 1]) {
    test(`fast ${preference} swipe (${fingerDirection}) keeps moving through lift recoil`, async () => {
      await page.locator('#librarySwipeDirection').evaluate((select, value) => {
        select.value = value;
        select.dispatchEvent(new Event('change'));
      }, preference);
      const albumDirection = preference === 'follow' ? -fingerDirection : fingerDirection;
      await down();
      await move(600 + fingerDirection * 45);
      await move(600 + fingerDirection * 110);
      await move(600 + fingerDirection * 175);
      await move(600 + fingerDirection * 161, 8);
      await up(4);
      const path = await page.evaluate(async () => {
        const positions = [state.pos];
        while (state.pos !== state.target) {
          await new Promise(resolve => requestAnimationFrame(resolve));
          positions.push(state.pos);
        }
        return positions;
      });
      assert.ok((path.at(-1) - 20) * albumDirection >= 1, 'destination ' + path.at(-1));
      assert.ok(path.every((pos, i) => i === 0 || (pos - path[i - 1]) * albumDirection >= 0));
    });
  }
}

test('a touch lift with a stale coordinate keeps the last sleeve position', async () => {
  await page.evaluate(() => {
    crate.addEventListener('pointerup', e => Object.defineProperty(e, 'clientX', { value: 600 }),
      { capture: true, once: true });
  });
  await down();
  await move(520);
  await move(440);
  await up(200);
  assert.equal(await settled(), 19);
});

test('a fast deliberate turn flings in the new direction', async () => {
  await down();
  await move(535);
  await move(435);
  await move(395);
  await move(445);
  await move(470);
  await up(4);
  assert.ok(await settled() > 20);
});

test('the crate claims native touch scrolling and accepts a real wheel immediately after lift', async () => {
  await page.evaluate(() => {
    document.addEventListener('touchstart', e => { window.touchClaimed = e.defaultPrevented; }, { once: true });
  });
  await page.mouse.move(600, 350);
  await down();
  await move(520);
  await up(200);
  assert.equal(await page.evaluate(() => window.touchClaimed), true);
  assert.equal(await page.evaluate(() => state.target), 19);
  await page.mouse.wheel(0, 70);
  await page.waitForFunction(() => state.target === 20);
  assert.equal(await settled(), 20);
});

test('EQ closes back to the sidebar and an interrupted dismissal can reopen it', async () => {
  const menu = page.locator('#menu'), eq = page.locator('#eq');
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=eq]').click();
  await eq.waitFor();
  await page.keyboard.press('Escape');
  assert.ok(await eq.evaluate(el => el.inert));
  assert.ok(await page.locator('[data-action=eq]').evaluate(el => el === document.activeElement));
  await page.keyboard.press('Escape');
  await page.locator('#menuBtn').dispatchEvent('click');
  await page.waitForFunction(() => document.querySelector('.menu-panel').getBoundingClientRect().right === innerWidth);
  assert.equal(await menu.evaluate(el => el.inert), false);
  assert.equal(await page.evaluate(() => scrollX), 0);
  await page.mouse.click(60, 60);
  await menu.waitFor({ state: 'hidden' });
  await eq.waitFor({ state: 'hidden' });
});

for (const direction of ['follow', 'opposite']) {
  test(`a 160 px touch flick glides three records with ${direction} selected`, async () => {
    await page.locator('#librarySwipeDirection').evaluate((select, value) => { select.value = value; }, direction);
    await down();
    await move(550);
    await move(495);
    await move(440);
    await up(16);
    assert.equal(await page.evaluate(() => state.target), direction === 'follow' ? 23 : 17);
  });
}

test('swiping the track list scrolls without a track-name tooltip or a playback change', async () => {
  await page.evaluate(() => {
    window.testTracks = ['Scratch N Sniff', "OWOL's Theme", 'Neko Soup'].map(title => ({ title, path: title }));
  });
  await down();
  await up();
  const rail = page.getByRole('navigation', { name: 'Album tracks' });
  const track = rail.getByRole('button', { name: /OWOL's Theme/ });
  await track.waitFor();
  const { x, y, width, height } = await track.boundingBox();
  const start = x + width / 2, middle = y + height / 2;
  await touch('touchStart', start, 16, middle);
  await move(start - 60, 32, middle);
  await move(start - 150, 32, middle);
  await up();
  await page.waitForFunction(() => document.getElementById('alTrack').scrollLeft > 0);
  assert.equal(await rail.getByRole('button', { name: /Scratch N Sniff/ }).getAttribute('aria-current'), 'true');
  assert.ok(await track.evaluate(el => !el.closest('[title]') && !el.querySelector('[title]')));
});

test('a direct track selection ignores end and error events from the previous track', async () => {
  await page.evaluate(() => {
    window.testTracks = ['One', 'Two', 'Three', 'Four'].map(title => ({ title, path: title }));
  });
  await down();
  await up();
  const tracks = page.locator('#alTrack button');
  await tracks.nth(3).waitFor();
  const first = await page.evaluate(() => state.playing.playbackId);
  await tracks.nth(3).click();
  await page.evaluate(id => {
    window.emitPlayback('track-ended', id);
    window.emitPlayback('playback-error', [id, 'previous track']);
  }, first);
  assert.equal(await tracks.nth(3).getAttribute('aria-current'), 'true');
  assert.equal(await page.locator('#toast.show').count(), 0);
  await tracks.nth(0).click();
  await page.evaluate(() => window.emitPlayback('track-ended', state.playing.playbackId));
  await page.waitForFunction(() => state.playing.index === 1);
  assert.equal(await tracks.nth(1).getAttribute('aria-current'), 'true');
});

test('accepting the sleeve behind the playing album keeps its image at the handoff', async () => {
  await down();
  await up();
  await page.locator('#alTrack button').waitFor();
  await page.locator('#backBtn').click();
  await page.locator('#album').waitFor({ state: 'hidden' });
  await page.keyboard.press('ArrowRight');
  await settled();
  await down();
  await up();
  const offer = page.locator('#proposedCover');
  await offer.waitFor();
  await page.evaluate(() => { window.offeredImage = document.querySelector('#proposedImg'); });
  await offer.press('Enter');
  await offer.waitFor({ state: 'hidden' });
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 21');
  assert.equal(await page.evaluate(() => window.offeredImage === document.querySelector('#coverImg')), true);
});

test('automatic painting is opt-in, persists its count and checks only once per playing album', async () => {
  assert.ok(await page.locator('#playImageCount').isDisabled());
  await page.evaluate(() => {
    sessionStorage.setItem('test.envKey', 'yes');
    document.querySelector('#generateOnPlay').click();
    const select = document.querySelector('#playImageCount');
    select.value = '5';
    select.dispatchEvent(new Event('change'));
  });
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  assert.equal(await page.locator('#playImageCount').inputValue(), '5');
  await down(); await up();
  await page.locator('#alTrack button').waitFor();
  const calls = await page.evaluate(() => {
    const id = state.playing.playbackId;
    window.emitPlayback('track-started', id - 1);
    window.emitPlayback('track-started', id);
    window.emitPlayback('track-started', id);
    return window.generationRequests;
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].onlyIfEmpty, true);
  assert.equal(calls[0].count, 5);
});

test('AI style is a default that an album can override, remember and restore', async () => {
  await page.evaluate(() => {
    localStorage.setItem('muso.autoStyle', '1');
    sessionStorage.setItem('test.envKey', 'yes');
  });
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await down(); await up();
  await page.locator('#albumMenuBtn').click();
  const style = page.locator('#albumStyle');
  assert.equal(await style.textContent(), 'Default · AI choice');
  assert.equal(await page.locator('#genLabel').textContent(), 'Generate with AI style');
  await style.click();
  const gallery = page.getByRole('dialog', {name: 'Atmosphere style'});
  await gallery.waitFor();
  assert.equal(await gallery.locator('#styleCanvas img').count(), 22);
  await gallery.locator('#styleCanvas img').evaluateAll(images => Promise.all(images.map(img => img.decode())));
  for (let i = 0; i < 8; i++) await page.keyboard.press('ArrowRight');
  assert.equal(await page.locator('#styleCaption').textContent(), 'Pixel Art');
  await gallery.getByRole('button', {name: 'Use this style'}).click();
  await page.getByRole('button', {name: 'Generate Pixel Art images'}).click();
  await page.waitForFunction(() => window.generationRequests?.[0]?.style === 'pixelart');

  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await down(); await up();
  await page.locator('#albumMenuBtn').click();
  assert.equal(await style.textContent(), 'Pixel Art');
  assert.equal(await page.locator('#aiStyle').textContent(), 'Pixel Art');
  assert.equal(await page.evaluate(() => albumStyle(state.albums[21].id)), 'auto');
  await style.click();
  assert.equal(await gallery.locator('[data-style=pixelart]').getAttribute('aria-pressed'), 'true');
  await page.keyboard.press('Escape');
  assert.ok(await style.evaluate(el => document.activeElement === el));
  assert.equal(await style.textContent(), 'Pixel Art');
  await style.click();
  await gallery.locator('#defaultStyleChoice').click();
  await page.getByRole('button', {name: 'Generate with AI style'}).click();
  await page.waitForFunction(() => window.generationRequests?.[0]?.style === 'auto');
  assert.equal(await page.evaluate(() => localStorage.getItem('muso.style.' + state.viewAlbum.id)), null);
});

test('the atmosphere toggle reflects saved images even with display turned off', async () => {
  await page.evaluate(() => localStorage.setItem('muso.atmos', '0'));
  await down(); await up();
  await page.locator('#albumMenuBtn').click();
  const toggle = page.locator('#showAtmosBtn');
  assert.ok(await toggle.isDisabled());
  await page.evaluate(() => window.emitPlayback('atmos-image-ready', {
    albumId: state.viewAlbum.id, style: 'ambient', files: ['atmos/first.jpg'],
  }));
  assert.ok(await toggle.isEnabled());
  assert.equal(await toggle.getAttribute('aria-pressed'), 'false');
  await page.evaluate(async () => {
    window.testPaintings = ['atmos/saved.jpg'];
    await startAtmospheres(state.viewAlbum.id);
  });
  assert.ok(await toggle.isEnabled());
  await page.evaluate(async () => { window.testPaintings = []; await startAtmospheres(state.viewAlbum.id); });
  assert.ok(await toggle.isDisabled());
});

test('the style gallery returns to AI settings and applies only a confirmed choice', async () => {
  await page.locator('#menuBtn').click();
  await page.getByRole('button', {name:'AI Features', exact:true}).click();
  const trigger = page.locator('#aiStyle');
  await trigger.focus();
  await page.keyboard.press('Space');
  const gallery = page.getByRole('dialog', {name:'Atmosphere style'});
  await gallery.waitFor();
  await gallery.getByRole('button', {name:'Close style selection'}).click();
  assert.equal(await trigger.textContent(), 'Default · Cinematic');
  assert.ok(await trigger.evaluate(el => document.activeElement === el));
  await trigger.click();
  await gallery.getByRole('button', {name:'Next style'}).click();
  await gallery.getByRole('button', {name:'Dreamscape', exact:true}).click();
  assert.equal(await trigger.textContent(), 'Dreamscape');
  assert.ok(await page.locator('#ai').isVisible());
  assert.equal(await page.evaluate(() => window.generationRequests?.length ?? 0), 0);
});

test('the style covers follow touch swipes and wheel browsing without changing the album', async () => {
  await down(); await up();
  await page.locator('#albumMenuBtn').click();
  await page.locator('#albumStyle').click();
  const stage = page.locator('#styleCanvas');
  const bounds = await stage.boundingBox();
  const x = bounds.x + bounds.width / 2, y = bounds.y + bounds.height / 2;
  await touch('touchStart', x, 16, y);
  await touch('touchMove', x + 48, 16, y);
  await touch('touchMove', x + 120, 32, y);
  await touch('touchEnd', null, 100);
  await page.waitForFunction(() => document.querySelector('#styleCaption').textContent !== 'Cinematic');
  const afterSwipe = await page.locator('#styleCaption').textContent();
  await page.mouse.move(x, y);
  await page.mouse.wheel(0, 80);
  await page.waitForFunction(old => document.querySelector('#styleCaption').textContent !== old, afterSwipe);
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('#albumStyle').textContent(), 'Default · Cinematic');
  assert.equal(await position(), 20);
  assert.equal(await page.evaluate(() => window.generationRequests?.length ?? 0), 0);
});

test('the cover stays up until the current atmosphere has decoded', async () => {
  // Use the HTTP form of Muso's image protocol so Playwright can serve a real image.
  await page.addInitScript(() => Object.defineProperty(navigator, 'userAgent', { value: 'Windows' }));
  await page.route('http://muso.localhost/atmos/**', route => route.fulfill({
    contentType: 'image/svg+xml',
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="32" height="16" fill="red"/></svg>',
  }));
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await page.evaluate(() => {
    window.testPaintings = ['atmos/first.svg'];
    window.decodes = [];
    const decode = HTMLImageElement.prototype.decode;
    HTMLImageElement.prototype.decode = async function () {
      await decode.call(this);
      if (this.closest('#atmosPictures')) await new Promise(resolve => window.decodes.push(resolve));
    };
  });
  await down();
  await up();
  await page.waitForFunction(() => window.decodes.length === 1);
  await page.evaluate(() => {
    window.emitPlayback('cover-intro-ended', state.playing.playbackId);
    window.testPaintings = ['atmos/replacement.svg'];
  });
  await page.locator('#albumMenuBtn').click();
  await page.locator('#albumStyle').click();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => window.decodes.length === 2);
  await page.evaluate(() => window.decodes[0]());
  assert.equal(await page.locator('#album.showing-atmos').count(), 0);
  await page.evaluate(() => window.decodes[1]());
  const painting = page.locator('#album.showing-atmos #atmosPictures img.on');
  await painting.waitFor();
  assert.ok(await page.locator('#showAtmosBtn').isEnabled());
  assert.match(await painting.getAttribute('src'), /replacement\.svg$/);
});

test('atmosphere grain keeps its pixel size across display scales and window sizes', async () => {
  await down();
  await up();
  await page.locator('#alTrack button').waitFor();
  for (const [width, height, density] of [[1200, 800, 1], [1280, 800, 2], [930, 620, 1.5]]) {
    await input.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: density, mobile: false });
    await page.waitForFunction(dpr => {
      const canvas = document.getElementById('atmosGrain');
      return devicePixelRatio === dpr && canvas.clientWidth >= innerWidth && canvas.clientHeight >= innerHeight &&
        canvas.width === Math.round(canvas.clientWidth * dpr) && canvas.height === Math.round(canvas.clientHeight * dpr);
    }, density);
  }
});

test('a queued record keeps the outgoing artwork until the incoming cover is ready', async () => {
  await page.evaluate(async () => {
    const nextAlbum = { ...state.albums[21], art: true };
    await startAlbum(state.albums[20], [nextAlbum]);
    window.previousImage = document.querySelector('#coverImg');
    window.previousImage.src = 'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="blue"/></svg>';
    window.previousImage.classList.remove('hidden');
    await window.previousImage.decode();
    const decode = HTMLImageElement.prototype.decode;
    HTMLImageElement.prototype.decode = function () {
      if (this.id === 'coverImg') return new Promise(resolve => { window.coverReady = resolve; });
      return decode.call(this);
    };
  });
  await page.locator('#nextBtn').click();
  const outgoing = page.locator('.outgoing-sleeve img');
  await outgoing.waitFor();
  assert.ok(await outgoing.evaluate(el => el === window.previousImage));
  const paused = await page.locator('.outgoing-backdrop').evaluate(el => getComputedStyle(el).animationPlayState);
  assert.equal(paused, 'paused');
  await page.evaluate(() => window.coverReady());
  await outgoing.waitFor({ state: 'detached' });
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 21');
});

test('a large crate finishes folding before its stack reappears', async () => {
  await page.evaluate(() => window.fillCrate(40));
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=crates]').click();
  const stack = page.locator('.shelf-crate[data-id="1"] .stack');
  await stack.click();
  await page.locator('#fanClose').click();
  const fan = page.locator('#crateFan');
  const lastDelay = await page.locator('.fan-cover').first().evaluate(el =>
    parseFloat(getComputedStyle(el).transitionDelay) * 1000);
  assert.ok(lastDelay <= 216);
  assert.ok(await stack.evaluate(el => el.classList.contains('lifted-away')));
  await fan.waitFor({ state: 'hidden' });
  assert.ok(await stack.isVisible());
});
