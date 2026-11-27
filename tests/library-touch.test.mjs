import assert from 'node:assert/strict';
import { after, before, beforeEach, afterEach, test } from 'node:test';
import { chromium } from 'playwright-core';

let browser, page, input, now;
before(async () => {
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
});
after(() => browser?.close());
beforeEach(async () => {
  const context = await browser.newContext({ viewport: { width: 1200, height: 800 }, hasTouch: true });
  page = await context.newPage();
  await page.addInitScript(() => {
    const albums = Array.from({ length: 80 }, (_, i) => ({
      id: i.toString(16).padStart(6, '0'), artist: 'Test', title: 'Record ' + i,
    }));
    localStorage.setItem('muso.libraryPosition', JSON.stringify({ index: 20, id: albums[20].id }));
    const events = new Map();
    window.testHeos = async () => ({players:[],groups:[],output:null});
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
        if (command.startsWith('heos_')) return window.testHeos(command,args);
        if (command === 'local_volume') {
          if (args?.value != null) {
            window.localVolume = args.value;
            if (args.save) window.savedVolume = args.value;
          }
          return window.localVolume ?? 100;
        }
        if (command === 'remove_folder') { window.removedMusicFolder = args.path; return []; }
        if (command === 'play_track' || command === 'set_runout') {
          (window.audioCommands ??= []).push({command, ...args});
          return;
        }
        if (command === 'get_folders') { await window.folderGate; return ['test']; }
        if (command === 'get_albums') return structuredClone(albums);
        if (command === 'upgrade_cover') {
          window.coverRequest = args;
          (window.coverSteps ??= []).push(args.step);
          return window.coverReplies?.[args.step.type] ?? window.coverResult;
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
          if (args.change?.type === 'reorder') {
            const {id, album, index} = args.change;
            const entries = crates.lists.find(c => c.id === id).albums;
            const [entry] = entries.splice(entries.findIndex(a => a.id === album), 1);
            entries.splice(index, 0, entry);
          }
          if (args.change?.type === 'rename') crates.lists.find(c => c.id === args.change.id).name = args.change.name;
          if (args.change?.type === 'add') {
            const list = crates.lists.find(c => c.id === args.change.id);
            if (list) list.albums.push({id: args.change.album});
            else crates.lists.push({ id: crates.lists.length + 1, name: 'Filed records', albums: [{ id: args.change.album }] });
          }
          return structuredClone(crates);
        }
        if (command === 'get_tracks') return window.testTracks ?? [{ path: 'test', title: 'Track' }];
        if (command === 'album_images') {
          (window.personalRequests ??= []).push(args);
          return {files:window.personalFiles ?? [],warning:null};
        }
        if (command === 'image_inventory') return {folders:[],total:0,pending:0,unreadable:0};
        if (command === 'get_atmospheres') return window.testSeries?.[args.style] ?? window.testPaintings ?? [];
        if (command === 'delete_atmosphere') {
          (window.deletedImages ??= []).push(args);
          window.testSeries[args.style] = window.testSeries[args.style].filter(file => file !== args.file);
          return;
        }
        if (command === 'painting_credit') return window.testCredits?.[args.file] ?? null;
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
afterEach(() => page?.context().close());

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
  assert.equal(await page.evaluate(() => window.coverRequest.step.offer_ai), true);
  await page.evaluate(async () => {
    window.coverResult = {status: 'no_match'};
    await findCovers([focused()]);
  });
  assert.equal(await page.evaluate(() => window.coverRequest.step.offer_ai), false);
});

test('MP3 cover writing is opt-in, persists, and reports tag errors after a cover upgrade', async () => {
  const run = async single => page.evaluate(async single => {
    window.coverResult = {status:'updated', file:'new.jpg', source:{provider:'Test'},
      tags: single ? {written:2, failed:1, first_error:'locked.mp3'} : {}};
    await findCovers([focused()], single);
    return window.coverRequest.writeMp3Covers;
  }, single);
  assert.equal(await run(false), false);
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=settings]').click();
  await page.locator('#writeMp3Covers').check();
  await page.reload();
  await page.locator('#crate .sleeve').first().waitFor();
  assert.equal(await run(true), true);
  assert.match(await page.locator('#coverProgress').textContent(), /2 MP3 tag\(s\) updated.*1 MP3 tag\(s\) failed.*locked.mp3/);
  assert.equal(await run(false), true);
});

test('the bottom fan browses a crate across navigation, playback and refresh, then restores the library', async () => {
  await page.evaluate(async () => {
    fillCrate(3);
    dateAlbum(2, Date.now()/1000);
    dateAlbum(19, Date.now()/1000);
    await loadAlbums();
    await albumCrates.load();
  });
  await page.locator('#cratePeek').tap();
  await page.getByRole('button', {name:'Records, 3 records'}).tap();
  assert.equal(await page.locator('#libraryCrateName').textContent(), 'Records');
  assert.equal(await page.locator('#counter').textContent(), '0001 / 0003');
  await page.locator('#newAlbums').click();
  assert.deepEqual(await page.evaluate(() => state.albums.map(a => a.title)), ['Record 2']);
  await page.locator('#newAlbums').click();
  await page.locator('#indexMode').click();
  await page.keyboard.press('ArrowRight');
  await settled();
  await down(); await up();
  await page.locator('#alTrack button').waitFor();
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 1');
  await page.locator('#backBtn').click();
  await page.evaluate(() => refreshLibrary());
  assert.equal(await page.locator('#counter').textContent(), '0002 / 0003');
  await page.locator('#closeLibraryCrate').click();
  assert.equal(await page.evaluate(() => focused().title), 'Record 20');
  assert.equal(await page.evaluate(() => state.albums.length), 80);
});

async function finishTrack() {
  await page.evaluate(() => emitPlayback('track-ended', state.playing.playbackId));
}

test('starting mid-crate follows saved order beyond five albums after leaving the crate', async () => {
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.evaluate(async () => {
    fillCrate(9);
    await invoke('library_crates', {change:{type:'reorder', id:1, album:'000007', index:2}});
    await albumCrates.load();
    window.testTracks = ['One', 'Two'].map(title => ({title, path:title}));
  });
  await page.locator('#cratePeek').tap();
  await page.locator('#browseCrates button').first().tap();
  await page.locator('#indexMode').click(); // Browse by artist, play in crate order.
  await page.keyboard.press('ArrowRight');
  await settled();
  await down(); await up();
  await page.locator('#alTrack button').first().waitFor();
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 1');
  await page.locator('#backBtn').click();
  await page.locator('#closeLibraryCrate').click();
  await page.locator('#nowPlaying').click();
  for (const title of ['Record 7', 'Record 2', 'Record 3', 'Record 4', 'Record 5', 'Record 6', 'Record 8']) {
    await finishTrack();
    await page.waitForFunction(() => state.playing.index === 1);
    await finishTrack();
    await page.waitForFunction(t => state.playing.album.title === t && state.playing.index === 0, title);
  }
  await finishTrack();
  await finishTrack();
  await page.waitForFunction(() => state.playing.ended);
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 8');
});

test('a queued crate selection carries its sequence and switching to a library album ends it', async () => {
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.evaluate(async () => { fillCrate(8); await albumCrates.load(); });
  await page.keyboard.press('Enter');
  await page.locator('#backBtn').click();
  await page.locator('#cratePeek').click();
  await page.locator('#browseCrates button').first().click();
  await page.locator('#crate').press('Enter');
  const waiting = page.locator('.queued-cover');
  await waiting.waitFor();
  assert.equal(await waiting.count(), 1);
  await finishTrack();
  await page.waitForFunction(() => state.playing.album.title === 'Record 0');
  assert.equal(await page.evaluate(() => state.playing.remaining.length), 7);
  await finishTrack();
  await page.waitForFunction(() => state.playing.album.title === 'Record 1');
  await page.locator('#backBtn').click();
  await page.locator('#closeLibraryCrate').click();
  await page.locator('#crate').press('Enter');
  await waiting.press('ArrowUp');
  await page.waitForFunction(() => state.playing.album.title === 'Record 20');
  await finishTrack();
  await page.waitForFunction(() => state.playing.ended);
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 20');
});

test('Order follows the crate editor and is the default each time a crate opens', async () => {
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.evaluate(async () => { fillCrate(3); await albumCrates.load(); });
  const mode = page.locator('#indexMode'), label = page.locator('#indexModeLabel');
  const titles = () => page.evaluate(() => state.albums.map(album => album.title));
  await mode.click();
  await mode.click();
  assert.equal(await label.textContent(), 'Genre');

  await page.locator('#cratePeek').click();
  await page.locator('#browseCrates button').first().click();
  assert.equal(await label.textContent(), 'Order');
  assert.ok(await page.locator('#indexLetters').isHidden());
  assert.ok(await page.locator('#indexGenres').isHidden());
  await page.locator('#editLibraryCrate').click();
  const covers = page.locator('#fanCovers .fan-cover');
  const first = await covers.first().boundingBox(), last = await covers.last().boundingBox();
  await page.mouse.move(last.x + last.width / 2, last.y + last.height / 2);
  await page.mouse.down();
  await page.mouse.move(first.x + first.width / 2, first.y + first.height / 2, {steps: 10});
  await page.mouse.up();
  await page.locator('#fanClose').click();
  const arranged = ['Record 2', 'Record 0', 'Record 1'];
  assert.deepEqual(await titles(), arranged);
  await page.evaluate(() => refreshLibrary());
  assert.deepEqual(await titles(), arranged);

  for (const name of ['Artist', 'Album', 'Genre', 'Order']) {
    await mode.click();
    assert.equal(await label.textContent(), name);
    if (name === 'Album') assert.deepEqual(await titles(), ['Record 0', 'Record 1', 'Record 2']);
  }
  assert.deepEqual(await titles(), arranged);
  await mode.click();
  await page.locator('#closeLibraryCrate').click();
  assert.equal(await label.textContent(), 'Genre');
  await page.locator('#cratePeek').click();
  await page.locator('#browseCrates button').first().click();
  assert.equal(await label.textContent(), 'Order');
  assert.equal(await page.locator('#capTitle').textContent(), 'Record 2');
});

test('crate settings open the current crate for editing and return to its records', async () => {
  const settings = page.getByRole('button', {name: 'Crate settings'});
  assert.ok(await settings.isHidden());
  await page.evaluate(async () => { fillCrate(3); await albumCrates.load(); });
  await page.locator('#cratePeek').click();
  await page.locator('#browseCrates button').first().click();
  await settings.focus();
  await page.keyboard.press('Space');
  await page.getByRole('heading', {name: 'Crate settings'}).waitFor();
  assert.equal(await page.locator('#fanName').textContent(), 'Records');
  assert.ok(await page.locator('#crateShelf').isHidden());
  await page.locator('#fanRename').click();
  await page.locator('#fanNameInput').fill('Ambient');
  await page.locator('#fanNameInput').press('Enter');
  await page.locator('#fanClose').click();
  assert.equal(await page.locator('#libraryCrateName').textContent(), 'Ambient');
  assert.equal(await page.locator('#counter').textContent(), '0001 / 0003');
  assert.ok(await settings.evaluate(el => el === document.activeElement));
});

test('crate sleeves fly from the chosen stack to their slots and return to its bottom preview', async () => {
  await page.evaluate(async () => { fillCrate(4); await albumCrates.load(); });
  await page.locator('#cratePeek').tap();
  const choice = page.locator('#browseCrates button').first();
  const passage = await choice.evaluate(async b => {
    await Promise.allSettled(b.parentNode.getAnimations({subtree:true}).map(a => a.finished));
    const centre = el => {
      const r = el.getBoundingClientRect();
      return [r.left + r.width / 2, r.top + r.height / 2];
    };
    const source = centre(b.querySelector('.cc[data-album-id="000000"]'));
    b.click();
    const record = document.querySelector('#crate .sleeve[data-index="0"]');
    const animation = record.getAnimations()[0];
    animation.pause();
    animation.currentTime = 0;
    const start = centre(record);
    animation.currentTime = 300;
    const lifted = centre(record);
    await Promise.all(crateMotion.flights.map(f => { f.finish(); return f.finished; }));
    const slot = centre(record);
    $('closeLibraryCrate').click();
    const back = record.getAnimations()[0];
    back.pause();
    back.currentTime = 0;
    const returning = centre(record);
    back.currentTime = back.effect.getComputedTiming().endTime;
    const landed = centre(record);
    const preview = centre(document.querySelector('#cratePeek .cc[data-album-id="000000"]'));
    for (const f of crateMotion.flights) f.finish();
    return {source, start, lifted, slot, returning, landed, preview};
  });
  const near = (a,b) => Math.hypot(a[0]-b[0], a[1]-b[1]) < 2;
  assert.ok(near(passage.start, passage.source));
  assert.ok(passage.lifted[1] < passage.start[1] && passage.lifted[1] > passage.slot[1]);
  assert.ok(near(passage.returning, passage.slot));
  assert.ok(near(passage.landed, passage.preview));
  await page.locator('.returning-sleeves').waitFor({state:'detached'});
  assert.equal(await page.evaluate(() => focused().title), 'Record 20');
});

test('reduced motion opens and closes a filled crate immediately', async () => {
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.evaluate(async () => { fillCrate(3); await albumCrates.load(); });
  await page.locator('#cratePeek').tap();
  await page.locator('#browseCrates button').first().tap();
  assert.equal(await page.locator('#crate').evaluate(el => el.getAnimations({subtree:true}).length), 0);
  await page.locator('#closeLibraryCrate').tap();
  assert.equal(await page.locator('.returning-sleeves').count(), 0);
  assert.equal(await page.evaluate(() => focused().title), 'Record 20');
});

test('empty crates survive a library refresh and can be closed with the keyboard', async () => {
  await page.evaluate(async () => { fillCrate(0); await albumCrates.load(); });
  await page.locator('#cratePeek').focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Space');
  await page.evaluate(() => refreshLibrary());
  assert.equal(await page.locator('#capTitle').textContent(), 'This crate is empty');
  assert.equal(await page.locator('#crate .sleeve').count(), 0);
  assert.ok(await page.locator('#onboard').isHidden());
  await page.keyboard.press('Enter');
  assert.equal(await page.evaluate(() => focused().title), 'Record 20');
});

test('cover research and using its match each require confirmation in the player', async () => {
  await page.route('https://coverartarchive.org/**', route => route.request().url().endsWith('/front')
    ? route.fulfill({ contentType:'image/svg+xml', body:'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="900"/>' })
    : route.fulfill({ status: 404 }));
  await page.evaluate(() => {
    window.coverReplies = {
      lookup: {status: 'offer_research'},
      research: {status: 'review_covers', choices: [{label: 'Test Artist — Test Album', location: {provider: 'music_brainz', id: 'confirmed-group'}}]},
      use_release: {status: 'already_large_enough'},
    };
  });
  await down(); await up();
  const start = async () => {
    await page.locator('#albumMenuBtn').click();
    await page.locator('#findAlbumCover').click();
  };
  const ask = page.getByRole('dialog', {name: 'Identify album with AI?'});
  const review = page.getByRole('dialog', {name: 'Use this release?'});
  await start();
  assert.match(await ask.innerText(), /one paid web search/);
  await ask.getByRole('button', {name: 'Cancel'}).click();
  assert.deepEqual(await page.evaluate(() => window.coverSteps.map(s => s.type)), ['lookup']);
  await start();
  await ask.getByRole('button', {name: 'Search with AI'}).click();
  assert.match(await review.innerText(), /Test Artist — Test Album/);
  await page.waitForFunction(() => document.querySelector('#coverPreview img')?.naturalWidth === 1000);
  const artwork = review.locator('#coverPreview img');
  assert.equal(await review.evaluate(el => getComputedStyle(el).backgroundColor), 'rgba(0, 0, 0, 0)');
  assert.equal(await review.locator('#coverBackdrop').getAttribute('src'), await artwork.getAttribute('src'));
  assert.ok((await artwork.boundingBox()).width >= 400);
  assert.equal(await artwork.evaluate(el => getComputedStyle(el).objectFit), 'contain');
  await page.setViewportSize({width: 390, height: 660});
  assert.ok((await artwork.boundingBox()).width >= 280);
  assert.ok(await review.evaluate(el => el.scrollWidth <= el.clientWidth));
  await page.keyboard.press('Escape');
  await review.waitFor({state:'hidden'});
  assert.deepEqual(await page.evaluate(() => window.coverSteps.map(s => s.type)), ['lookup', 'lookup', 'research']);
  await start();
  await ask.getByRole('button', {name: 'Search with AI'}).click();
  await review.getByRole('button', {name: 'Use cover'}).click();
  await page.waitForFunction(() => window.coverRequest.step.type === 'use_release');
  assert.equal(await page.evaluate(() => window.coverRequest.step.location.id), 'confirmed-group');
});

test('the player keeps cover research progress visible until it finishes and retains the checked sources', async () => {
  await down(); await up();
  await page.evaluate(() => {
    window.coverResult = new Promise(resolve => { window.finishCoverLookup = resolve; });
  });
  await page.locator('#albumMenuBtn').click();
  await page.locator('#findAlbumCover').click();
  const progress = page.locator('#coverSearchStatus');
  for (const source of ['MusicBrainz', 'TheAudioDB']) {
    await page.evaluate(source => window.emitPlayback('cover-search-progress', {
      albumId: state.viewAlbum.id, source, message: 'Checking ' + source,
    }), source);
    assert.equal(await progress.innerText(), 'Checking ' + source);
  }
  await page.clock.install();
  await page.clock.fastForward(10000);
  assert.ok(await progress.isVisible());
  await page.evaluate(() => window.finishCoverLookup({status:'no_front_cover'}));
  await progress.waitFor({state:'hidden'});
  await page.clock.fastForward(10000);
  await page.locator('#albumMenuBtn').click();
  assert.match(await page.locator('#coverSearchResult').innerText(), /no front cover.*MusicBrainz, TheAudioDB/i);
});

test('choosing among cover editions uses the selected provider without a paid search', async () => {
  await page.route('https://r2.theaudiodb.com/**', route => route.request().url().endsWith('/2.jpg')
    ? route.fulfill({ contentType:'image/svg+xml', body:'<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800"/>' })
    : route.fulfill({ status: 404 }));
  await page.evaluate(() => {
    const choices = ['2001', '2020'].map((year, i) => ({
      label: 'Album · ' + year,
      location: {provider:'the_audio_db', id:String(i+1), image:`https://r2.theaudiodb.com/images/media/album/thumb/${i+1}.jpg`},
    }));
    window.coverReplies = { lookup:{status:'review_covers', choices}, use_release:{status:'already_large_enough'} };
  });
  await down(); await up();
  await page.locator('#albumMenuBtn').click();
  await page.locator('#findAlbumCover').click();
  const dialog = page.getByRole('dialog', {name:'Choose a front cover'});
  const use = dialog.getByRole('button', {name:'Use cover'});
  assert.ok(await use.isDisabled());
  await dialog.getByText('Cover preview unavailable').waitFor();
  await dialog.getByRole('radio', {name:/2020/}).check();
  await page.waitForFunction(() => document.querySelector('#coverPreview img')?.naturalWidth === 800);
  assert.equal(await dialog.locator('#coverPreview img').getAttribute('alt'), 'Album · 2020');
  await use.click();
  await page.waitForFunction(() => window.coverRequest.step.type === 'use_release');
  assert.deepEqual(await page.evaluate(() => window.coverSteps.map(step => step.type)), ['lookup','use_release']);
  assert.equal(await page.evaluate(() => window.coverRequest.step.location.id), '2');
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
  assert.equal(await mode.getAttribute('title'), await mode.getAttribute('aria-description'));
  const highlighted = await color();
  assert.notEqual(highlighted, rest);
  await mode.tap();
  assert.equal(await page.locator('#indexModeLabel').textContent(), 'Album');
  assert.equal(await color(), rest);
  assert.equal(await page.locator('[title]').count(), 0);
  assert.match(await mode.getAttribute('aria-description'), /Switch to/);
  const current = page.locator('#indexLetters [aria-current=true]');
  const scale = () => current.evaluate(el => Number(getComputedStyle(el).getPropertyValue('--letter-scale')));
  assert.equal(await scale(), 1);
  await page.keyboard.press('Tab');
  assert.ok(await current.evaluate(el => el.matches(':focus-visible')));
  assert.equal(await scale(), 1.8);
  await mode.hover({ position: { x: 5, y: 5 } });
  assert.equal(await color(), highlighted);
  assert.equal(await mode.getAttribute('title'), await mode.getAttribute('aria-description'));
});

async function setApiKey(dialog, value) {
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=' + dialog + ']').click();
  await page.locator('#' + dialog + ' input[type=password]').fill(value);
  await page.locator('#' + dialog).getByRole('button', {name:'Done', exact:true}).click();
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
  await page.getByRole('button', { name: 'Close settings' }).click();
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
  await page.getByRole('button', { name: 'Close settings' }).click();
  await down();
  await move(540);
  await up(220);
  assert.equal(await settled(), 18);
});

for (const [id, title] of [['settings', 'Settings'], ['ai', 'AI Features']]) {
  test(title + ' keeps its header visible and its controls usable with touch and keyboard', async () => {
    await page.setViewportSize({width:820, height:560});
    await page.locator('#menuBtn').tap();
    await page.locator('[data-action=' + id + ']').tap();
    const dialog = page.getByRole('dialog', {name:title, exact:true});
    const close = dialog.getByRole('button', {name:'Close ' + title.toLowerCase()});
    await close.waitFor();
    assert.ok(await close.evaluate(el => el === document.activeElement));
    const bounds = await close.boundingBox();
    const sizes = await dialog.evaluate(el => [...el.querySelectorAll('button, input, select, summary')].map(control => {
      const r = control.getBoundingClientRect();
      return [control.id || control.textContent.trim(), r.width, r.height];
    }));
    for (const [name, width, height] of sizes) {
      assert.ok(width >= 44 && height >= 44, name + ' has a touch target of ' + width + ' × ' + height);
    }
    assert.ok(await dialog.locator('.modal-scroll').evaluate(el => el.scrollWidth === el.clientWidth));
    const switches = dialog.getByRole('switch');
    assert.ok(await switches.count() >= 3);
    for (const toggle of await switches.all()) {
      assert.ok(await toggle.evaluate(el => Math.abs(el.getBoundingClientRect().right - el.labels[0].getBoundingClientRect().right) < 1));
    }
    const toggle = switches.first();
    const before = await toggle.isChecked();
    await toggle.locator('..').locator('span').tap();
    assert.equal(await toggle.isChecked(), !before);
    await toggle.focus();
    await page.keyboard.press('Space');
    assert.equal(await toggle.isChecked(), before);
    await close.focus();
    await page.keyboard.press('Shift+Tab');
    assert.ok(await dialog.getByRole('button', {name:'Done'}).evaluate(el => el === document.activeElement));
    assert.deepEqual(await close.boundingBox(), bounds);
    await page.keyboard.press('Tab');
    assert.ok(await close.evaluate(el => el === document.activeElement));
    await dialog.locator('input[type=password]').focus();
    await page.keyboard.press('Escape');
    await dialog.waitFor({state:'hidden'});
    assert.ok(await page.locator('#menuBtn').evaluate(el => el === document.activeElement));
  });
}

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
  const dismiss = page.getByRole('button', {name:'Close settings'});
  const before = await dismiss.boundingBox();
  assert.equal(await page.getByRole('button', {name:'Remove music folder test'}).textContent(), 'Remove');
  await page.locator('#settings .modal-actions .close').scrollIntoViewIfNeeded();
  assert.ok(await scroll.evaluate(el => el.scrollTop > 0));
  assert.deepEqual(await dismiss.boundingBox(), before);
  await dismiss.tap();
  await page.locator('#settings').waitFor({ state: 'hidden' });
  assert.equal(await page.evaluate(() => window.removedMusicFolder), undefined);
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
    await page.locator('#coverImg').evaluate(async img => {
      img.src = 'style-previews/ambient.webp';
      img.classList.remove('hidden');
      await img.decode();
    });
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
      const ghost = document.querySelector('.crate-ghost');
      const soft = ghost.querySelector('.soft-cover');
      const blurred = getComputedStyle(soft || ghost).filter;
      return +getComputedStyle(ghost).opacity < .4 && parseFloat(blurred.slice(5)) > 5 &&
        (!soft || +getComputedStyle(soft).opacity > .95);
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

test('the sidebar fits a short window and restores its headings when enlarged', async () => {
  await page.locator('#menuBtn').click();
  const panel = page.locator('.menu-panel');
  for (const height of [560, 700, 800]) {
    await page.setViewportSize({ width: 820, height });
    assert.equal(await panel.locator('h2:visible').count(), height === 800 ? 3 : 0);
    assert.ok(await panel.evaluate(el => el.scrollHeight <= el.clientHeight));
    assert.equal(await panel.getByRole('group', {name:'Library'}).count(), 1);
  }
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

test('five queued sleeves survive browsing, promote individually and advance in order', async () => {
  await page.evaluate(async () => {
    await openAlbum(state.albums[20]);
    for (const i of [21, 22, 23, 24, 25, 26, 22]) openAlbum(state.albums[i]);
  });
  const covers = page.locator('.queued-cover');
  assert.equal(await covers.count(), 5);
  assert.equal(await page.locator('.cover-stack.fanned').count(), 1);
  await page.locator('#backBtn').click();
  await page.locator('#album').waitFor({state:'hidden'});
  await page.locator('#nowPlaying').click();
  const third = covers.nth(2);
  const box = await third.boundingBox();
  const x = box.x + box.width - 12, y = box.y + box.height / 2;
  assert.equal(await page.evaluate(([x,y]) => document.elementFromPoint(x,y).closest('button')?.getAttribute('aria-label'), [x,y]), await third.getAttribute('aria-label'));
  await touch('touchStart', x, 16, y);
  await move(x, 90, y - 115);
  await up();
  for (const title of ['Record 23', 'Record 21', 'Record 22', 'Record 24', 'Record 25']) {
    await page.waitForFunction(t => state.playing.album.title === t, title);
    await page.evaluate(() => window.emitPlayback('track-ended', state.playing.playbackId));
  }
  await page.waitForFunction(() => state.playing.ended);
  assert.equal(await covers.count(), 0);
});

for (const [count, slot] of [[1,0], [5,0], [2,1], [5,2], [5,3], [5,4]]) {
  test(`lifting waiting sleeve ${slot + 1} of ${count} brings it in from above as the playing sleeve fades down`, async () => {
    await page.evaluate(async ({count, slot}) => {
      await openAlbum(state.albums[20]);
      for (const album of state.albums.slice(21, 21 + count)) openAlbum(album);
      window.previousImage = document.getElementById('coverImg');
      window.nextImage = albumQueue[slot].button.querySelector('img');
    }, {count, slot});
    const waiting = page.locator('.queued-cover');
    const titles = await waiting.locator('.queue-title').allTextContents();
    const box = await waiting.nth(slot).boundingBox();
    const x = box.x + box.width - 10, y = box.y + box.height / 2;
    await touch('touchStart', x, 16, y);
    await move(x, 90, y - 115);
    await up();
    const old = page.locator('.promoting-record .outgoing-sleeve');
    await old.waitFor();
    const frames = await old.evaluate(outgoing => {
      const incoming = document.getElementById('cover');
      if (outgoing.firstElementChild !== window.previousImage || incoming.firstElementChild !== window.nextImage) throw Error('Lost the sleeve artwork');
      const sample = (element, name) => {
        const animation = element.getAnimations().find(a => a.animationName === name);
        animation.pause();
        return [0, .5, 1].map(fraction => {
          animation.currentTime = fraction * animation.effect.getTiming().duration;
          const style = getComputedStyle(element);
          return {y:new DOMMatrix(style.transform).m42, opacity:Number(style.opacity)};
        });
      };
      return [sample(outgoing, 'queue-cover-out'), sample(incoming, 'queue-cover-in')];
    });
    const [exit, enter] = frames;
    assert.ok(exit[0].y < exit[1].y && exit[1].y < exit[2].y && exit[0].opacity > exit[1].opacity && exit[2].opacity === 0);
    assert.ok(enter[0].y < enter[1].y && enter[1].y < 0 && enter[2].y === 0 && enter[1].opacity > 0 && enter[2].opacity === 1);
    await old.waitFor({state:'detached'});
    assert.equal(await page.locator('#alTitle').textContent(), titles[slot]);
    assert.deepEqual(await waiting.locator('.queue-title').allTextContents(), titles.filter((_, index) => index !== slot));
    if (count === 1) return;
    await page.emulateMedia({reducedMotion:'reduce'});
    await waiting.first().press('ArrowUp');
    await page.waitForFunction(title => document.querySelector('#alTitle').textContent === title, titles[slot === 0 ? 1 : 0]);
    assert.equal(await page.locator('.outgoing-sleeve').count(), 0);
  });
}

test('swiping a waiting record down removes only that record and re-arms the final groove', async () => {
  const playing = await page.evaluate(async () => {
    document.querySelector('#vinylSounds').click();
    await openAlbum(state.albums[20]);
    for (const i of [21, 22, 23]) openAlbum(state.albums[i]);
    return state.playing.playbackId;
  });
  const sleeves = page.locator('.queued-cover');
  await page.locator('.cover-stack').evaluate(async stack => {
    await new Promise(requestAnimationFrame);
    await Promise.all(stack.getAnimations({ subtree:true }).filter(a => a instanceof CSSTransition).map(a => a.finished));
  });
  const middle = await sleeves.nth(1).boundingBox();
  const x = middle.x + middle.width - 10, y = middle.y + middle.height / 2;
  await touch('touchStart', x, 16, y);
  await move(x, 40, y + 30);
  const opacity = () => sleeves.nth(1).evaluate(el => Number(getComputedStyle(el).opacity));
  const partial = await opacity();
  assert.ok(partial > 0 && partial < 1);
  await move(x, 40, y + 12);
  await page.waitForFunction(initial => Number(getComputedStyle(document.querySelectorAll('.queued-cover')[1]).opacity) > initial, partial);
  await up();
  await page.waitForFunction(() => getComputedStyle(document.querySelectorAll('.queued-cover')[1]).opacity === '1');
  assert.equal(await sleeves.count(), 3);
  await touch('touchStart', x, 16, y);
  await move(x, 90, y + 115);
  await up();
  assert.deepEqual(await sleeves.locator('.queue-title').allTextContents(), ['Record 21', 'Record 23']);
  assert.equal(await page.evaluate(() => state.playing.playbackId), playing);
  await sleeves.first().press('ArrowDown');
  await sleeves.first().press('ArrowDown');
  assert.equal(await sleeves.count(), 0);
  assert.deepEqual(await page.evaluate(() => window.audioCommands.at(-1)), {
    command:'set_runout', playbackId:playing, seconds:120, changeAtEnd:false
  });
});

test('vinyl settings persist and select a single drop, change or finite runout at album boundaries', async () => {
  assert.equal(await page.locator('#vinylSounds').isChecked(), false);
  const minutes = page.locator('#runoutMinutes');
  assert.equal(await minutes.inputValue(), '2');
  await page.locator('#menuBtn').tap();
  await page.locator('[data-action=settings]').tap();
  assert.ok(await page.locator('#runoutLess').isDisabled());
  await page.getByRole('switch', { name: 'Needle and groove sounds' }).tap();
  await page.locator('#runoutMore').tap();
  assert.equal(await page.evaluate(() => localStorage.getItem('muso.runoutSeconds')), '150');
  await page.locator('#runoutLess').tap();
  assert.equal(await minutes.inputValue(), '2');
  await minutes.fill('1,5');
  await minutes.press('Enter');
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  assert.equal(await page.locator('#vinylSounds').isChecked(), true);
  await page.evaluate(async () => {
    window.testTracks = ['a','b'].map(path => ({path, title:path}));
    await openAlbum(state.albums[20]);
    await next();
    openAlbum(state.albums[21]);
    window.emitPlayback('record-changing', state.playing.playbackId);
  });
  await page.waitForFunction(() => state.playing.album.title === 'Record 21');
  const calls = await page.evaluate(() => window.audioCommands);
  assert.deepEqual(calls.map(x => x.command), ['play_track','play_track','set_runout','play_track']);
  assert.equal(calls[0].vinylLead, 'drop');
  assert.equal(calls[1].vinylLead, null);
  assert.equal(calls[1].runoutSeconds, 90);
  assert.equal(calls[2].changeAtEnd, true);
  assert.equal(calls[2].seconds, 0);
  assert.equal(calls[3].afterPlaybackId, calls[1].playbackId);
  assert.equal(calls[3].vinylLead, 'change');
  await page.evaluate(async () => {
    await next();
    window.emitPlayback('runout-started', state.playing.playbackId);
  });
  await page.locator('#runoutLabel').waitFor();
  await page.evaluate(() => window.emitPlayback('track-ended', state.playing.playbackId));
  await page.waitForFunction(() => state.playing.ended);
  await page.locator('#runoutLabel').waitFor({state:'hidden'});
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
  const offer = page.locator('.queued-cover');
  await offer.waitFor();
  await page.evaluate(() => { window.offeredImage = document.querySelector('.queued-cover img'); });
  await offer.press('Enter');
  await offer.waitFor({ state: 'hidden' });
  assert.equal(await page.locator('#alTitle').textContent(), 'Record 21');
  assert.equal(await page.evaluate(() => window.offeredImage === document.querySelector('#coverImg')), true);
});

test('automatic painting is opt-in, persists its count and checks only once per playing album', async () => {
  assert.ok(await page.locator('#playImageCount').isDisabled());
  assert.equal(await page.locator('#repaintToggle').isChecked(), false);
  assert.equal(await page.locator('#artistPersonas').isChecked(), false);
  await page.evaluate(() => {
    sessionStorage.setItem('test.envKey', 'yes');
    document.querySelector('#generateOnPlay').click();
    document.querySelector('#repaintToggle').click();
    document.querySelector('#artistPersonas').click();
    const select = document.querySelector('#playImageCount');
    select.value = '5';
    select.dispatchEvent(new Event('change'));
  });
  await page.locator('#menuBtn').tap();
  await page.locator('[data-action="ai"]').tap();
  await page.locator('#imageModel').selectOption('sunburst');
  await page.locator('#imageResolution').selectOption('4k');
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
  assert.equal(calls[0].allowRepaint, true);
  assert.equal(calls[0].artistPersonas, true);
  assert.deepEqual(calls[0].imageSettings, {model:'sunburst', resolution:'4k'});
});

test('image settings have touch-sized selectors and apply to manual generation', async () => {
  await page.setViewportSize({ width: 390, height: 700 });
  await page.evaluate(() => sessionStorage.setItem('test.envKey', 'yes'));
  await page.locator('#menuBtn').tap();
  await page.locator('[data-action="ai"]').tap();
  const model = page.getByRole('combobox', {name:'Image model', exact:true});
  const resolution = page.getByRole('combobox', {name:'Resolution', exact:true});
  assert.equal(await model.inputValue(), 'flare');
  assert.equal(await resolution.inputValue(), '1080p');
  for (const control of [model, resolution]) {
    const box = await control.boundingBox();
    assert.ok(box.height >= 44 && box.width >= 44 && box.x >= 0 && box.x + box.width <= 390);
  }
  await resolution.selectOption('4k');
  await page.locator('#aiGenerateBtn').tap();
  await page.waitForFunction(() => window.generationRequests?.length === 1);
  assert.deepEqual(await page.evaluate(() => window.generationRequests[0].imageSettings),
    {model:'flare', resolution:'4k'});
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
  await page.locator('#stylePicker').waitFor({state:'hidden'});
  assert.ok(await style.evaluate(el => document.activeElement === el));
  assert.equal(await style.textContent(), 'Pixel Art');
  await style.click();
  await gallery.locator('#defaultStyleChoice').click();
  await page.getByRole('button', {name: 'Generate with AI style'}).click();
  await page.waitForFunction(() => window.generationRequests?.[0]?.style === 'auto');
  assert.equal(await page.evaluate(() => localStorage.getItem('muso.style.' + state.viewAlbum.id)), null);
});

test('album choices align on the right and both galleries fade over the blurred player', async () => {
  await page.locator('#crate .sleeve[data-index="20"]').click();
  await page.locator('#albumMenuBtn').click();
  const rightEdges = await page.locator('#albumMenu .style-field > :is(button, select)').evaluateAll(
    fields => fields.map(field => field.getBoundingClientRect().right));
  assert.equal(rightEdges.length, 3);
  assert.ok(Math.max(...rightEdges) - Math.min(...rightEdges) < 1);

  for (const id of ['albumStyle', 'albumPersona']) {
    const result = await page.locator('#' + id).evaluate(async trigger => {
      trigger.click();
      const dialog = document.getElementById(trigger.getAttribute('aria-controls'));
      function sampleFade() {
        const fade = dialog.getAnimations().find(a => a.transitionProperty === 'opacity');
        fade.pause();
        fade.currentTime = fade.effect.getTiming().duration / 2;
        const opacity = Number(getComputedStyle(dialog).opacity);
        fade.finish();
        return opacity;
      }
      const opening = sampleFade();
      const background = getComputedStyle(dialog).backgroundColor;
      const blur = getComputedStyle(dialog, '::backdrop').backdropFilter;
      const closed = new Promise(resolve => dialog.addEventListener('close', resolve, {once:true}));
      dialog.querySelector('header button').click();
      const closing = sampleFade();
      await closed;
      return {opening, closing, background, blur, focused: document.activeElement === trigger};
    });
    assert.ok(result.opening > 0 && result.opening < 1);
    assert.ok(result.closing > 0 && result.closing < 1);
    assert.equal(result.background, 'rgba(0, 0, 0, 0)');
    assert.equal(result.blur, 'blur(20px)');
    assert.ok(result.focused);
  }
});

test('artist portraits commission the selected persona per album and preserve the style', async () => {
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=ai]').click();
  await page.locator('#artistPersonas').check();
  await page.locator('#aiPersona').click();
  const gallery = page.getByRole('dialog', {name: 'Artist persona'});
  const portraits = gallery.locator('img');
  assert.equal(await portraits.count(), 6);
  await portraits.evaluateAll(images => Promise.all(images.map(img => img.decode())));
  await page.emulateMedia({reducedMotion:'reduce'});
  const canvas = gallery.locator('#personaCanvas');
  await canvas.hover();
  await page.mouse.wheel(0, 70);
  await page.waitForFunction(() => document.querySelector('#personaCaption').textContent === 'Experimental material artist');
  await canvas.press('ArrowRight');
  assert.equal(await gallery.locator('#personaCaption').textContent(), 'Editorial illustrator');
  assert.equal(await page.locator('#aiPersona').textContent(), 'Default · Random artist');
  await gallery.getByRole('button', {name: 'Use this artist'}).click();
  await page.evaluate(() => {
    sessionStorage.setItem('test.envKey', 'yes');
    localStorage.setItem('muso.style.' + focused().id, 'pixelart');
  });
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').click();
  await page.locator('#albumMenuBtn').click();
  const trigger = page.locator('#albumPersona');
  assert.equal(await trigger.textContent(), 'Editorial illustrator');
  assert.equal(await page.evaluate(() => albumPersona(state.albums[21].id)), 'random');
  await trigger.click();
  assert.equal(await gallery.getByRole('button', {pressed: true}).getAttribute('value'), 'Editorial illustrator');
  await page.keyboard.press('Escape');
  assert.ok(await trigger.evaluate(el => el === document.activeElement));
  await page.getByRole('button', {name: 'Generate Pixel Art images'}).click();
  await page.waitForFunction(() => window.generationRequests?.length);
  const [request] = await page.evaluate(() => window.generationRequests);
  assert.equal(request.artistPersona, 'Editorial illustrator');
  assert.equal(request.artistPersonas, false);
  assert.equal(request.style, 'pixelart');
  await page.locator('#albumMenuBtn').click();
  await trigger.click();
  await gallery.getByRole('button', {name: 'No persona', exact: true}).click();
  assert.equal(await page.evaluate(() => albumPersona(state.viewAlbum.id)), 'none');
  await trigger.click();
  await gallery.locator('#personaDefault').click();
  assert.equal(await page.evaluate(() => albumPersona(state.viewAlbum.id)), 'random');
});

test('reopening a saved style keeps the covers fanned out after the first cover is hidden', async () => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.locator('#crate .sleeve[data-index="20"]').click();
  await page.locator('#albumMenuBtn').click();
  const trigger = page.locator('#albumStyle');
  await trigger.click();
  await page.keyboard.press('End');
  const covers = page.locator('#styleCanvas .sleeve');
  assert.ok(await covers.first().isHidden());
  await page.locator('#useStyle').click();

  for (const width of [1200, 640]) {
    await page.setViewportSize({ width, height: 800 });
    await trigger.click();
    assert.equal(await covers.last().getAttribute('aria-pressed'), 'true');
    const [far, near, selected] = await covers.evaluateAll(sleeves => sleeves.slice(-3).map(el => {
      const { left, width } = el.getBoundingClientRect();
      return { center: left + width / 2, width };
    }));
    assert.ok(near.center - far.center > selected.width * 0.05, 'side covers are spaced apart');
    assert.ok(selected.center - near.center > selected.width * 0.3, 'selected cover is distinct from its neighbour');
    await page.keyboard.press('Escape');
  }
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
  await page.locator('#stylePicker').waitFor({state:'hidden'});
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

for (const preference of ['opposite', 'follow']) {
  test(`style covers can be caught and tapped with the ${preference} swipe preference`, async () => {
    await down(); await up();
    await page.evaluate(pref => {
      document.querySelector('#librarySwipeDirection').value = pref;
      localStorage.setItem('muso.style.' + state.viewAlbum.id, 'pixelart');
    }, preference);
    await page.locator('#albumMenuBtn').click();
    await page.locator('#albumStyle').click();
    const stage = page.locator('#styleCanvas');
    const bounds = await stage.boundingBox();
    const x = bounds.x + bounds.width / 2, y = bounds.y + bounds.height / 2;
    const sign = preference === 'follow' ? -1 : 1;
    const offset = () => stage.locator('[data-style=pixelart]').evaluate(el => el.style.transform);
    await page.evaluate(() => document.addEventListener('touchstart', e => {
      window.styleTouchClaimed = e.defaultPrevented;
    }, {once:true}));
    await touch('touchStart', x, 16, y);
    await touch('touchMove', x + sign * 55, 16, y);
    await touch('touchMove', x + sign * 125, 16, y);
    await touch('touchEnd', null, 4);
    assert.equal(await page.evaluate(() => window.styleTouchClaimed), true);
    assert.ok(await page.evaluate(() => {
      const index = Object.values(styleNames).indexOf(document.querySelector('#styleCaption').textContent);
      return index > Object.keys(styleNames).indexOf('pixelart');
    }));
    await touch('touchStart', x, 16, y);
    const caught = await offset();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await offset(), caught);
    await touch('touchEnd', null, 120);
    assert.ok(await page.locator('#stylePicker').isVisible());
    assert.equal(await page.locator('#albumStyle').textContent(), 'Pixel Art');
    await page.waitForFunction(() => [...document.querySelectorAll('#styleCanvas .sleeve:not([hidden])')].some(el =>
      el.style.transform === 'translate3d(0px, 0px, 0px) rotateY(0deg)'));
    const name = await page.locator('#styleCaption').textContent();
    await touch('touchStart', x, 16, y);
    await touch('touchEnd', null, 16);
    await page.locator('#stylePicker').waitFor({state:'hidden'});
    assert.equal(await page.locator('#albumStyle').textContent(), name);
  });
}

test('delete in the player targets the confirmed painting across slides and protects personal images', async () => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'userAgent', {value:'Windows'}));
  await page.route('http://muso.localhost/**', route => route.fulfill({contentType:'image/svg+xml',
    body:'<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>'}));
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await page.evaluate(() => {
    window.testSeries = {tape:['atmos/first.jpg','atmos/second.jpg']};
    window.personalFiles = ['personal/1-1'];
  });
  await down(); await up();
  await page.waitForFunction(() => playlist?.shown === 'atmos/first.jpg');
  await page.locator('#albumMenuBtn').click();
  const remove = page.getByRole('button', {name:'Delete this atmosphere…'});
  assert.ok(await remove.isHidden()); // The track is still showing its cover.
  await page.evaluate(() => emitPlayback('cover-intro-ended', state.playing.playbackId));
  await remove.click();
  const dialog = page.getByRole('dialog', {name:'Delete this atmosphere?'});
  await dialog.getByRole('button', {name:'Cancel'}).click();
  assert.equal(await page.evaluate(() => window.deletedImages?.length ?? 0), 0);
  await page.evaluate(() => enqueue(state.albums[21]));
  await page.locator('#albumMenuBtn').click();
  await remove.click();
  await page.evaluate(() => loadPainting(playlist, 'atmos/second.jpg'));
  assert.match(await dialog.locator('img').getAttribute('src'), /first\.jpg$/);
  await dialog.getByRole('button', {name:'Delete image'}).click();
  await page.waitForFunction(() => playlist?.files.length === 1 && playlist?.shown === 'atmos/second.jpg');
  assert.deepEqual(await page.evaluate(() => window.deletedImages), [{albumId:'000014',style:'tape',file:'atmos/first.jpg'}]);
  await page.evaluate(() => { removeQueued(albumQueue[0]); updateAtmosView(); });
  await page.locator('#albumMenuBtn').click();
  await remove.click();
  await dialog.getByRole('button', {name:'Delete image'}).click();
  await page.waitForFunction(() => playlist?.shown === 'personal/1-1');
  await page.locator('#albumMenuBtn').click();
  assert.ok(await remove.isHidden());
  assert.ok(await page.locator('#openImageFolder').isVisible());
});

test('the cover waits for a decoded atmosphere and fades back with every queued sleeve on the next track', async () => {
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
    window.testCredits = {'atmos/replacement.svg':{title:'Record 20 · 01', style:'Tape Decay', persona:'Graphic designer'}};
    window.testTracks = [{path:'first', title:'First'}, {path:'second', title:'Second'}];
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
  await page.evaluate(() => {
    for (const album of state.albums.slice(21, 24)) enqueue(album);
    layoutQueue();
    window.decodes[1]();
  });
  const painting = page.locator('#album.showing-atmos #atmosPictures img.on');
  await painting.waitFor();
  assert.ok(await page.locator('#showAtmosBtn').isEnabled());
  assert.match(await painting.getAttribute('src'), /replacement\.svg$/);
  const credit = page.locator('#paintingCredit');
  await credit.waitFor();
  assert.equal(await page.locator('#paintingTitle').textContent(),'Record 20 · 01');
  assert.equal(await page.locator('#paintingByline').textContent(),'Tape Decay · Graphic designer');
  const stack = page.locator('.cover-stack');
  const stackX = await stack.evaluate(el => el.getBoundingClientRect().x);
  await page.mouse.move(100,100);
  await page.mouse.move(350,180, {steps:8});
  assert.ok(await stack.evaluate(el => el.inert), 'hover leaves the atmosphere on screen');
  await page.mouse.click(100,100);
  assert.ok(await stack.evaluate(el => !el.inert));
  assert.equal(await stack.evaluate(el => el.getBoundingClientRect().x), stackX);
  await painting.waitFor();
  await page.touchscreen.tap(100,100);
  assert.ok(await stack.evaluate(el => !el.inert));
  await painting.waitFor();
  await page.locator('#album').evaluate(el => el.classList.add('idle'));
  await credit.waitFor({state:'hidden'});
  await page.locator('#album').evaluate(el => el.classList.remove('idle'));
  await credit.waitFor();
  const midFade = await page.evaluate(async () => {
    const stack = document.querySelector('.cover-stack');
    const layers = [document.querySelector('.atmos'), stack];
    await Promise.all(layers.flatMap(el => el.getAnimations()
      .filter(a => a instanceof CSSTransition).map(a => a.finished)));
    if (!stack.inert || Number(getComputedStyle(stack).opacity) !== 0) throw Error('Queue should be hidden in Atmosphere');
    window.emitPlayback('track-ending', state.playing.playbackId - 1);
    if (!stack.inert) throw Error('An old playback cue changed the cover');
    window.emitPlayback('track-ending', state.playing.playbackId);
    for (const layer of layers) {
      const fade = layer.getAnimations().find(a => a.transitionProperty === '--motion-opacity');
      fade.pause();
      fade.currentTime = 900;
    }
    if (stack.inert) throw Error('Queue should be interactive in the cover view');
    const stackOpacity = Number(getComputedStyle(stack).opacity);
    return [Number(getComputedStyle(layers[0]).opacity), ...Array.from(stack.querySelectorAll('#cover, .queued-cover'), el => {
      const style = getComputedStyle(el);
      return style.visibility === 'visible' ? stackOpacity * Number(style.opacity) : 0;
    })];
  });
  assert.equal(await page.locator('#alTrack [aria-current]').textContent(), '01First');
  assert.equal(await stack.evaluate(el => el.getBoundingClientRect().x), stackX);
  await page.evaluate(() => window.emitPlayback('track-ended', state.playing.playbackId));
  assert.equal(await page.locator('#alTrack [aria-current]').textContent(), '02Second');
  assert.equal(midFade.length, 5);
  assert.ok(midFade.every(opacity => opacity > .4 && opacity < .6), 'the painting, main sleeve and three queued sleeves are visible halfway through the return fade');
});

test('rising pigment follows its vector rim at full HD and at high pixel density', async () => {
  await page.setViewportSize({width:1920,height:1080});
  await page.evaluate(() => {
    localStorage.setItem('muso.spectrum','1');
    localStorage.setItem('muso.eqStyle','pigment');
    localStorage.setItem('muso.eqColour','#ffffff');
    localStorage.setItem('muso.eqIntensity','100');
    localStorage.setItem('muso.eqHeight','100');
    const view = createSpectrumView(async () => {}, (_, receive) => { window.drawBands = receive; }, null);
    view.setPlayback(true, 1);
    const pigment = document.getElementById('spectrumPigment');
    document.body.append(pigment);
    Object.assign(pigment.style, {position:'fixed', zIndex:9999});
    document.body.style.background = '#000';
    document.body.querySelectorAll(':scope > :not(#spectrumPigment):not(#album)').forEach(el => { el.style.visibility = 'hidden'; });
    // Keep the SVG clipping definitions in the render tree.
    document.getElementById('album').classList.remove('hidden');
    document.getElementById('album').style.opacity = '0';
  });
  for (const density of [1, 2]) {
    await input.send('Emulation.setDeviceMetricsOverride', {width:1920,height:1080,deviceScaleFactor:density,mobile:false});
    const ys = await page.evaluate(async () => {
      window.drawBands({payload:[1,[1,.9,.3,.1,.8,.1,.4,.6,.7,.1,1,.6,.5,.2,.1,.9]]});
      await new Promise(requestAnimationFrame);
      const path = document.getElementById('eqTrace');
      return Array.from({length:30}, (_, i) => {
        const x = (40 + i + .5) / innerWidth * 1000;
        let lo = 0, hi = path.getTotalLength();
        for (let j=0; j<20; j++) {
          const mid = (lo + hi) / 2;
          if (path.getPointAtLength(mid).x < x) lo = mid; else hi = mid;
        }
        return path.getPointAtLength(hi).y / 100 * innerHeight;
      });
    });
    const {data} = await input.send('Page.captureScreenshot', {format:'png', clip:{x:40,y:0,width:30,height:1080,scale:1}});
    const errors = await page.evaluate(async ({data,ys,density}) => {
      const img = await createImageBitmap(await (await fetch('data:image/png;base64,' + data)).blob());
      const canvas = document.createElement('canvas');
      canvas.width = img.width; canvas.height = img.height;
      const ctx = canvas.getContext('2d');
      ctx.drawImage(img,0,0);
      const pixels = ctx.getImageData(0,0,img.width,img.height).data;
      return ys.map((y, i) => {
        const x = i*density + Math.floor(density/2);
        let last = -1;
        for (let row = 0; row < img.height; row++) {
          if (pixels[4*(row*img.width+x)] > 100) last = row;
        }
        return Math.abs(last + .5 - y*density);
      });
    }, {data,ys,density});
    assert.ok(Math.max(...errors) < 3, `Painted edge deviates by ${Math.max(...errors).toFixed(2)} device pixels at ${density}x (y=${ys[0]}, first error=${errors[0]}).`);
  }
});

test('atmosphere grain keeps its pixel size across display scales and window sizes', async () => {
  await down();
  await up();
  await page.locator('#alTrack button').waitFor();
  await page.evaluate(() => {
    const album = document.querySelector('#album');
    album.dataset.filter = 'film';
    album.classList.add('showing-atmos');
  });
  assert.equal(await page.locator('#atmosGrain').evaluate(el => getComputedStyle(el).animationName), 'film-grain');
  for (const [width, height, density] of [[1200, 800, 1], [1280, 800, 2], [930, 620, 1.5]]) {
    await input.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: density, mobile: false });
    await page.waitForFunction(dpr => {
      const canvas = document.getElementById('atmosGrain');
      return devicePixelRatio === dpr && canvas.clientWidth >= innerWidth && canvas.clientHeight >= innerHeight &&
        canvas.width === Math.round(canvas.clientWidth * dpr) && canvas.height === Math.round(canvas.clientHeight * dpr);
    }, density);
  }
});

test('light, haze and grain move on visible atmospheres and respect reduced motion', async () => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'userAgent', { value: 'Windows' }));
  await page.route('http://muso.localhost/atmos/filter.svg', route => route.fulfill({
    contentType: 'image/svg+xml',
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60"><rect width="80" height="60" fill="#888"/></svg>',
  }));
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await page.evaluate(() => { window.testPaintings = ['atmos/filter.svg']; });
  await down(); await up();
  await page.locator('#alTrack button').waitFor();
  await page.evaluate(() => emitPlayback('cover-intro-ended', state.playing.playbackId));
  await page.locator('#album.showing-atmos #atmosPictures img.on').waitFor();
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.atmos')).opacity === '1');
  const region = {x: 212, y: 412, width: 96, height: 64};
  for (const [filter, layer] of [['light','lightFilter'], ['haze','lightFilter'], ['film','atmosGrain']]) {
    await page.evaluate(async ([filter, layer]) => {
      filterSelect.value = filter;
      filterSelect.dispatchEvent(new Event('change'));
      const animation = $(layer).getAnimations()[0];
      await animation.ready;
      if (animation.playState !== 'running') throw Error('Filter is paused');
      animation.pause();
      animation.currentTime = 0;
    }, [filter, layer]);
    const first = await page.screenshot({clip:region});
    await page.evaluate(([filter, layer]) => {
      $(layer).getAnimations()[0].currentTime = filter === 'film' ? 270 : 12000;
      $(layer).getAnimations()[0].play();
    }, [filter, layer]);
    assert.ok(!first.equals(await page.screenshot({clip:region})), filter + ' changes the rendered image');
  }
  await page.mouse.move(215, 415);
  await page.waitForFunction(() => $('atmosPictures').classList.contains('refracting') && Number($('waveDepth').getAttribute('scale')) > 0);
  await page.emulateMedia({reducedMotion:'reduce'});
  assert.equal(await page.locator('#atmosGrain').evaluate(el => el.getAnimations().length), 0);
  await page.evaluate(() => { filterSelect.value = 'light'; applyEffects(); });
  assert.ok(await page.locator('#lightFilter').isHidden());
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

test('audio output lists rooms and groups, applies grouping explicitly and routes to the leader', async () => {
  await page.evaluate(() => {
    const snapshot = {host:'192.0.2.4',output:null,groups:[],players:[
      {pid:-12,name:'Kitchen',model:'Denon Home 150'}, {pid:34,name:'Library',model:'Denon CEOL'},
      {pid:56,name:'Bedroom',model:'Denon Home 150'}
    ]};
    window.heosWrites = [];
    window.testHeos = async (cmd,args) => {
      if (cmd === 'heos_group') {
        window.heosWrites.push(args.members);
        snapshot.groups = args.members.length === 1 ? [] : [{gid:args.members[0],name:'Library group',
          players:args.members.map((pid,i)=>({pid,role:i?'member':'leader'}))}];
      }
      if (cmd === 'heos_output') snapshot.output = args.pid;
      return structuredClone(snapshot);
    };
  });
  await page.locator('#menuBtn').tap();
  await page.getByRole('button',{name:'Audio output',exact:true}).tap();
  const ui = page.getByRole('dialog',{name:'Audio output'});
  assert.equal(await ui.getByRole('button',{name:/This computer/}).getAttribute('aria-pressed'),'true');
  await ui.getByRole('button',{name:'Create group'}).tap();
  await page.getByLabel('Main room').selectOption('34');
  await page.getByRole('checkbox',{name:'Kitchen'}).check();
  assert.deepEqual(await page.evaluate(()=>window.heosWrites),[]);
  await page.getByRole('button',{name:'Save group'}).tap();
  const target = ui.locator('.heos-target').filter({hasText:'Library group'});
  await target.tap();
  assert.equal(await target.getAttribute('aria-pressed'),'true');
  await ui.getByRole('button',{name:'Edit group Library group'}).tap();
  assert.equal(await page.getByLabel('Main room').isDisabled(),true);
  await page.getByRole('checkbox',{name:'Bedroom'}).check();
  await page.getByRole('button',{name:'Save group'}).tap();
  await ui.getByRole('button',{name:'Edit group Library group'}).tap();
  await page.getByRole('button',{name:'Ungroup all'}).tap();
  assert.equal(await ui.locator('.heos-target').filter({hasText:'Library'}).getAttribute('aria-pressed'),'true');
  await ui.getByRole('button',{name:/This computer/}).tap();
  assert.deepEqual(await page.evaluate(()=>window.heosWrites),[[34,-12],[34,-12,56],[34]]);
  await ui.getByRole('button',{name:'Close audio output'}).tap();
  await ui.waitFor({state:'hidden'});
});

test('HEOS shows an accessible green spinner during discovery and connection, then shows the result', async () => {
  await page.evaluate(() => {
    window.testHeos = () => new Promise((resolve, reject) => {
      window.finishHeos = resolve;
      window.failHeos = reject;
    });
  });
  await page.locator('#menuBtn').tap();
  await page.getByRole('button',{name:'Audio output',exact:true}).tap();
  const status = page.locator('#heosStatus');
  const searching = page.locator('#heosRefresh .heos-spinner');
  await searching.waitFor();
  assert.equal(await searching.evaluate(el => getComputedStyle(el).borderTopColor), 'rgb(95, 203, 134)');
  assert.equal(await status.textContent(), 'Looking for HEOS speakers…');
  await page.evaluate(() => finishHeos({players:[{pid:1,name:'Desk'},{pid:2,name:'Kitchen'}],groups:[],output:null}));
  await page.getByRole('button',{name:'Desk',exact:true}).tap();
  const row = page.locator('.heos-row').filter({hasText:'Desk'});
  const connecting = row.locator('.heos-spinner');
  await connecting.waitFor();
  assert.equal(await status.textContent(), 'Switching audio output…');
  assert.equal(await status.locator('.heos-spinner').count(), 0);
  const rowBox = await row.boundingBox(), indicatorBox = await connecting.boundingBox();
  assert.ok(Math.abs(rowBox.x + rowBox.width - indicatorBox.x - indicatorBox.width) < 1);
  assert.equal(await page.locator('#audioDialog .heos-spinner').count(), 1);
  await page.evaluate(() => failHeos('The speaker did not fetch the audio stream.'));
  await page.waitForFunction(() => document.getElementById('heosStatus').textContent.includes('did not fetch'));
  assert.equal(await status.getByRole('img').count(), 0);
  assert.equal(await connecting.count(), 0);
  assert.equal(await page.getByRole('button',{name:/This computer/}).getAttribute('aria-pressed'), 'true');
});

test('the player speaker shortcut stays reachable across the hover gap and on touch', async () => {
  await page.evaluate(() => {
    window.testHeos = async () => ({players:[{pid:8,name:'Studio'}],groups:[],output:null});
  });
  await down(); await up();
  const volume = page.locator('#volumeToggle'), speakers = page.locator('#speakerSelect');
  await volume.hover();
  await speakers.waitFor();
  await speakers.evaluate(el => Promise.all(el.getAnimations().map(animation => animation.finished)));
  const left = await speakers.boundingBox(), right = await volume.boundingBox();
  const gap = (left.x + left.width + right.x) / 2, y = right.y + right.height / 2;
  await page.mouse.move(gap, y, {steps:8});
  assert.ok(await speakers.isVisible());
  await page.locator('#album').evaluate(el => el.classList.add('idle'));
  assert.equal(await page.locator('#playerAudio').evaluate(async el => {
    await Promise.all(el.getAnimations().map(animation => animation.finished));
    return getComputedStyle(el).opacity;
  }), '1');
  await page.mouse.move(100,100);
  await speakers.waitFor({state:'hidden'});
  assert.equal(await speakers.evaluate(el => {
    const matrix = new DOMMatrix(getComputedStyle(el).transform);
    return matrix.m41;
  }), right.x + right.width / 2 - left.x - left.width / 2);
  await volume.hover();
  await speakers.click();
  const room = page.getByRole('button',{name:'Studio',exact:true});
  await room.waitFor();
  const panel = page.locator('#audioDialog');
  const bounds = await panel.boundingBox();
  assert.ok(Math.abs(bounds.x + bounds.width - left.x - left.width) < 1);
  assert.ok(Math.abs(bounds.y + bounds.height + 12 - left.y) < 1);
  assert.equal(await panel.evaluate(el => getComputedStyle(el, '::backdrop').backdropFilter), 'blur(20px)');
  assert.equal(await panel.evaluate(el => getComputedStyle(el).backgroundColor), 'rgba(0, 0, 0, 0)');
  await page.mouse.click(100,100);
  await panel.waitFor({state:'hidden'});

  await page.touchscreen.tap(100,100);
  await speakers.waitFor({state:'hidden'});
  await volume.tap();
  await speakers.waitFor();
  assert.ok(await page.getByRole('slider',{name:'Muso volume'}).isEnabled());
  await speakers.tap();
  await room.waitFor();
  assert.equal(await volume.getAttribute('aria-expanded'), 'false');
  await page.locator('#audioClose').tap();
  await page.waitForFunction(() => document.activeElement.id === 'volumeToggle');
  await volume.press('Tab');
  assert.equal(await page.evaluate(() => document.activeElement.id), 'speakerSelect');
  await speakers.press('Space');
  await room.waitFor();
  await page.setViewportSize({width:820,height:560});
  const compact = await panel.boundingBox();
  assert.ok(compact.y >= 15 && compact.y + compact.height <= 560 - 80);
  await page.keyboard.press('Escape');
  await panel.waitFor({state:'hidden'});
  await page.locator('#backBtn').click();
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=audio-output]').click();
  await room.waitFor();
  const libraryPanel = await panel.boundingBox();
  assert.ok(Math.abs(libraryPanel.x + libraryPanel.width / 2 - 410) <= 1);
});

test('the speaker shortcut stays hidden when discovery finds no rooms', async () => {
  await page.evaluate(() => {
    window.testHeos = async () => ({players:[],groups:[],output:null});
  });
  await down(); await up();
  await page.locator('#volumeToggle').tap();
  await page.waitForFunction(() => document.getElementById('heosManual').open);
  assert.ok(await page.locator('#speakerSelect').isHidden());
  assert.ok(await page.getByRole('slider',{name:'Muso volume'}).isEnabled());
});

test('corner volume expands upwards and controls local output with touch and keyboard', async () => {
  await down(); await up();
  const button = page.locator('#volumeToggle'), capsule = page.locator('#playerVolume');
  await button.waitFor();
  const waves = button.locator('.volume-waves path:visible');
  assert.equal(await waves.count(), 3);
  await page.locator('#album').evaluate(el => Promise.all(el.getAnimations().map(a => a.finished)));
  const bottom = () => capsule.evaluate(el => el.offsetTop + el.offsetHeight);
  const before = await bottom();
  await button.tap();
  const slider = page.getByRole('slider',{name:'Muso volume'});
  await page.waitForFunction(() => document.querySelector('#playerVolume').offsetHeight === 200);
  const after = await capsule.boundingBox();
  assert.ok(Math.abs(before - await bottom()) <= 1);
  assert.ok(Math.abs(after.width - 48) < 1);
  assert.equal(await slider.inputValue(), '100');
  const fillColour = () => capsule.evaluate(el => {
    const ctx = document.createElement('canvas').getContext('2d');
    ctx.fillStyle = getComputedStyle(el, '::before').backgroundColor;
    ctx.fillRect(0, 0, 1, 1);
    return Array.from(ctx.getImageData(0, 0, 1, 1).data).slice(0, 3);
  });
  const [red, green, blue] = await fillColour();
  assert.ok(red > 100 && red < 170 && red > blue && blue > green && green < 15);
  await slider.press('Home');
  assert.equal(await waves.count(), 0);
  assert.ok(await button.locator('.volume-mute').isVisible());
  await slider.press('PageUp');
  assert.equal(await waves.count(), 1);
  await slider.press('End');
  for (let i = 0; i < 4; i++) await slider.press('PageDown');
  assert.equal(await waves.count(), 2);
  await slider.press('Home');
  await slider.press('PageUp');
  assert.equal(await slider.inputValue(), '10');
  const quiet = await fillColour();
  assert.ok(quiet[0] > quiet[1] && Math.max(...quiet) - Math.min(...quiet) < 30);
  const x = after.x + after.width / 2;
  await page.touchscreen.tap(x, after.y + after.height * .75);
  const low = Number(await slider.inputValue());
  assert.ok(low > 15 && low < 35);
  await slider.press('ArrowUp');
  await page.waitForFunction(v => window.savedVolume === v, low + 1);
  await slider.press('Escape');
  assert.equal(await button.getAttribute('aria-expanded'), 'false');
  assert.equal(await waves.count(), 1);
  assert.equal(await button.getAttribute('aria-description'), (low + 1) + '%');
  await button.press('Enter');
  assert.equal(await slider.inputValue(), String(low + 1));
  await page.touchscreen.tap(100,100);
  assert.equal(await button.getAttribute('aria-expanded'), 'false');
});

test('player menu exposes HEOS volume only on HEOS and controls a group or an individual room', async () => {
  await page.evaluate(() => {
    const rooms = {players:[{pid:-2,name:'Desk'},{pid:4,name:'Kitchen'}],groups:[
      {gid:9,name:'Both rooms',players:[{pid:-2,role:'leader'},{pid:4,role:'member'}]}],output:null};
    const levels = {session:'123',controls:[
      {id:9,group:true,name:'Both rooms',value:23},{id:-2,group:false,name:'Desk',value:19},{id:4,group:false,name:'Kitchen',value:27}]};
    window.volumeWrites = [];
    window.testHeos = async (command,args) => {
      if (command === 'heos_output') rooms.output = args.pid;
      if (command === 'heos_levels') return structuredClone(levels);
      if (command === 'heos_set_level') {
        window.volumeWrites.push(args);
        levels.controls.find(c => c.id === args.id && c.group === args.group).value = args.value;
        return structuredClone(levels);
      }
      return structuredClone(rooms);
    };
  });
  assert.ok(await page.locator('#albumHeos').evaluate(el => el.classList.contains('hidden')));
  await page.locator('#menuBtn').tap();
  await page.locator('[data-action=audio-output]').tap();
  await page.locator('.heos-target').filter({hasText:'Both rooms'}).tap();
  await page.locator('#audioClose').tap();
  await down(); await up();
  await page.locator('#volumeToggle[data-waves="1"]').waitFor();
  assert.equal(await page.locator('#volumeToggle').getAttribute('aria-description'), '23%');
  await page.locator('#volumeToggle').tap();
  const quick = page.getByRole('slider',{name:'Volume · Both rooms'});
  assert.equal(await quick.inputValue(),'23');
  await quick.press('ArrowUp');
  await quick.press('Escape');
  await page.evaluate(async () => { await playerVolume.settled(); });
  assert.ok(await page.evaluate(() => window.volumeWrites.every(w => w.id === 9 && w.group && w.value === 24) && window.volumeWrites.length > 0));
  await page.evaluate(() => { window.volumeWrites = []; });
  await page.locator('#albumMenuBtn').tap();
  await page.getByRole('button',{name:'HEOS volume', exact:true}).tap();
  const dialog = page.getByRole('dialog',{name:'HEOS volume'});
  const kitchen = dialog.getByRole('slider',{name:'Kitchen'});
  assert.equal(await kitchen.inputValue(),'27');
  assert.deepEqual(await page.evaluate(() => window.volumeWrites),[]);
  await kitchen.press('ArrowLeft');
  await dialog.getByRole('slider',{name:'Group · Both rooms'}).press('ArrowRight');
  await page.waitForFunction(() => window.volumeWrites.length === 2);
  assert.deepEqual(await page.evaluate(() => window.volumeWrites),[
    {session:'123',id:4,group:false,value:26},{session:'123',id:9,group:true,value:25}]);
  await page.keyboard.press('Escape');
  assert.ok(await dialog.isHidden());
  await page.locator('#albumMenuBtn').tap();
  await page.getByRole('button',{name:'HEOS volume', exact:true}).tap();
  await page.evaluate(() => window.emitPlayback('heos-disconnected'));
  assert.ok(await dialog.isHidden());
  await page.locator('#albumMenuBtn').tap();
  assert.ok(await page.locator('#albumHeos').isHidden());
});

test('TV follows the playing track while the controller stays in the library', async () => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'userAgent', {value:'Windows'}));
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  await page.evaluate(() => {
    const primary = { name:'Laptop', position:{x:0,y:0}, size:{width:1920,height:1080} };
    const tv = { name:'TV', position:{x:-1920,y:0}, size:{width:1920,height:1080} };
    Object.assign(window.__TAURI__.window, { availableMonitors:async () => [primary,tv], currentMonitor:async () => primary });
    window.testTracks = [{title:'First', artist:'Guest', path:'a'}, {title:'Second',path:'b'}];
    window.testPaintings = ['atmos/tv.svg'];
  });
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=presenter]').click();
  assert.equal(await page.locator('#tvDisplay').inputValue(), '[-1920,0]');
  await page.clock.install();
  await page.locator('#tvStart').click();
  const tv = await page.context().newPage();
  await tv.addInitScript(() => {
    window.__TAURI__ = {
      core:{invoke:async (cmd,args) => { if (cmd === 'set_spectrum_enabled') window.meterEnabled = args.enabled; }},
      event:{listen:async (event,receive) => { if (event === 'audio-spectrum') window.sendBands = id => receive({payload:[id,Array(16).fill(.7)]}); }},
    };
  });
  await page.context().route('http://muso.localhost/atmos/tv.svg', route => route.fulfill({
    contentType:'image/svg+xml', body:'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><rect width="64" height="32" fill="navy"/></svg>',
  }));
  await tv.goto(new URL('../ui/presenter.html', import.meta.url).href);
  await page.waitForFunction(() => tvReady);
  await page.evaluate(() => openAlbum(focused()));
  await tv.waitForFunction(() => document.querySelector('#title').textContent === 'First');
  assert.equal(await page.evaluate(() => state.view), 'library');
  assert.equal(await tv.locator('#artist').textContent(), 'Guest');
  await page.evaluate(() => document.getElementById('spectrumToggle').click());
  await tv.waitForFunction(() => window.meterEnabled && !!document.querySelector('#tvPictures img.on'));
  const firstId = await page.evaluate(() => state.playing.playbackId);
  const saver = page.locator('#librarySaver');
  const savedPosition = await position();
  await page.mouse.move(30, 50);
  await page.clock.fastForward(59000);
  assert.equal(await saver.isVisible(), false);
  await page.mouse.move(32, 50);
  await page.clock.fastForward(59000);
  assert.equal(await saver.isVisible(), false);
  await page.clock.fastForward(1000);
  await saver.locator('img.on').waitFor();
  assert.equal(await saver.locator('img.on').getAttribute('src'), await tv.locator('#tvPictures img.on').getAttribute('src'));
  await page.touchscreen.tap(600, 350);
  await saver.waitFor({state:'hidden'});
  assert.equal(await position(), savedPosition);
  assert.equal(await page.evaluate(() => state.view), 'library');
  await page.locator('#menuBtn').click();
  await page.clock.fastForward(61000);
  assert.equal(await saver.isVisible(), false);
  await page.keyboard.press('Escape');
  await page.clock.fastForward(60000);
  await saver.waitFor();
  await page.mouse.move(50, 60);
  await saver.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(() => state.playing.playbackId), firstId);
  await tv.evaluate(id => window.sendBands(id), firstId);
  await tv.waitForFunction(() => Number(getComputedStyle(document.getElementById('spectrumLine')).opacity) > 0);
  await page.evaluate(() => {
    const style = document.getElementById('eqStyle');
    style.value = 'pigment';
    style.dispatchEvent(new Event('change'));
  });
  await tv.waitForFunction(() => current.spectrum.style === 'pigment');
  await tv.evaluate(id => window.sendBands(id), firstId);
  await tv.waitForFunction(() => Number(getComputedStyle(document.getElementById('spectrumPigment')).opacity) > 0);
  await page.evaluate(() => togglePlay());
  await tv.waitForFunction(() => !window.meterEnabled && getComputedStyle(document.getElementById('spectrumLine')).opacity === '0');
  await page.evaluate(() => togglePlay());
  await tv.waitForFunction(() => window.meterEnabled);
  await page.evaluate(() => window.emitPlayback('cover-intro-ended', state.playing.playbackId));
  await tv.waitForFunction(() => !document.querySelector('aside').classList.contains('visible'));
  await page.evaluate(() => next());
  await tv.waitForFunction(() => document.querySelector('#title').textContent === 'Second');
  assert.equal(await tv.locator('#artist').textContent(), 'Test');
  await tv.evaluate(async id => { window.sendBands(id); await new Promise(requestAnimationFrame); }, firstId);
  assert.equal(await tv.locator('#spectrumLine').evaluate(el => getComputedStyle(el).opacity),'0');
  await page.evaluate(() => {
    const style = document.getElementById('eqStyle');
    style.value = 'echo';
    style.dispatchEvent(new Event('change'));
  });
  await tv.waitForFunction(() => current.spectrum.style === 'echo' && getComputedStyle(document.getElementById('eqEchoes')).display !== 'none');
  await page.evaluate(() => document.getElementById('spectrumToggle').click());
  await tv.waitForFunction(() => !window.meterEnabled);
  assert.ok(await tv.locator('aside').evaluate(el => el.classList.contains('visible')));
  const playingId = await page.evaluate(() => state.playing.playbackId);
  const offer = page.locator('.queued-cover');
  await page.evaluate(() => openAlbum(state.albums[21]));
  await offer.waitFor();
  await offer.locator('.swipe-hint').tap();
  assert.equal(await page.evaluate(() => state.playing.playbackId), playingId);
  assert.equal(await tv.locator('#title').textContent(), 'Second');
  await page.locator('#backBtn').tap();
  await page.locator('#album').waitFor({state:'hidden'});
  assert.equal(await page.evaluate(() => state.playing.playbackId), playingId);
  await page.evaluate(() => openAlbum(state.albums[21]));
  await offer.waitFor();
  await offer.evaluate(el => Promise.all(el.getAnimations().map(a => a.finished)));
  const box = await offer.boundingBox();
  const x = box.x + box.width - 22, y = box.y + box.height / 2;
  await touch('touchStart', x, 16, y);
  await move(x, 120, y - 100);
  await up();
  await tv.waitForFunction(() => document.querySelector('#title').textContent === 'First');
  await page.locator('#album').waitFor({state:'hidden'});
  assert.equal(await page.evaluate(() => state.playing.album.title), 'Record 21');
  assert.equal(await page.evaluate(() => state.view), 'library');
  await page.clock.fastForward(60000);
  await saver.waitFor();
  await page.evaluate(() => window.emitPlayback('presenter-closed'));
  await saver.waitFor({state:'hidden'});
  await page.clock.fastForward(61000);
  assert.equal(await saver.isVisible(), false);
  await tv.close();
});

test('TV and Images dialogs stay hidden after closing by touch, Escape or backdrop', async () => {
  await page.evaluate(() => {
    const monitor = {name:'Screen',position:{x:0,y:0},size:{width:1280,height:720}};
    Object.assign(window.__TAURI__.window, {availableMonitors:async()=>[monitor],currentMonitor:async()=>monitor});
  });
  for (const [action, id, done] of [['presenter','tvDialog','tvCancel'],['images','imagesDialog','imageDone']]) {
    const dialog = page.locator('#' + id);
    assert.equal(await dialog.isVisible(), false);
    for (const method of ['button','escape','backdrop']) {
      await page.locator('#menuBtn').tap();
      await page.locator('[data-action=' + action + ']').tap();
      await dialog.waitFor();
      if (method === 'button') await page.locator('#' + done).tap();
      if (method === 'escape') await page.keyboard.press('Escape');
      if (method === 'backdrop') await page.touchscreen.tap(5,5);
      await dialog.waitFor({state:'hidden'});
      assert.equal(await dialog.evaluate(el=>el.open),false);
    }
  }
});

test('personal images are shared by both views and generated album sets take priority', async () => {
  const result = await page.evaluate(async()=>{
    window.personalFiles=['personal/1-1','personal/3-1'];
    const id=focused().id;
    const [player,tv]=await Promise.all([savedPaintingSet(id,'cinematic'),savedPaintingSet(id,'auto')]);
    window.testPaintings=['atmos/album.jpg'];
    const generated=await savedPaintingSet(id,'cinematic');
    return {player,tv,generated,requests:window.personalRequests};
  });
  assert.equal(result.player.style,'personal');
  assert.deepEqual(result.player.files,result.tv.files);
  assert.deepEqual(result.generated.files,['atmos/album.jpg']);
  assert.equal(result.requests.length,1);
  assert.equal(result.requests[0].matching,false);
});

test('Images can mix the shared folder selection through the saved series and remember the choice', async () => {
  await page.locator('#menuBtn').click();
  await page.locator('[data-action=images]').click();
  const mix = page.locator('#imageMix');
  assert.equal(await mix.isChecked(), false);
  await mix.check();
  await page.reload();
  await page.locator('#crate .sleeve[data-index="20"]').waitFor();
  assert.ok(await mix.isChecked());
  const result = await page.evaluate(async () => {
    window.testPaintings = ['atmos/1', 'atmos/2', 'atmos/3'];
    window.personalFiles = Array.from({length: 5}, (_, i) => 'personal/' + i);
    const id = focused().id;
    const [player, tv] = await Promise.all([savedPaintingSet(id, 'auto'), savedPaintingSet(id, 'auto')]);
    document.getElementById('imageMix').click();
    const plain = await savedPaintingSet(id, 'auto');
    window.testPaintings = [];
    const fallback = await savedPaintingSet(id, 'auto');
    return {player, tv, plain, fallback, requests: window.personalRequests.length};
  });
  assert.deepEqual(result.player.files, ['atmos/1','personal/0','personal/1','atmos/2','personal/2','atmos/3','personal/3','personal/4']);
  assert.deepEqual(result.tv, result.player);
  assert.deepEqual(result.plain.files, ['atmos/1','atmos/2','atmos/3']);
  assert.equal(result.player.style, 'auto');
  assert.equal(result.fallback.files.length, 5);
  assert.equal(result.requests, 2);
});

test('clearing search removes the results and keeps typing in the field', async () => {
  await page.keyboard.type('R');
  const field = page.getByRole('textbox', {name:'Search the crate'});
  const clear = page.getByRole('button', {name:'Clear search'});
  await clear.tap();
  assert.equal(await field.inputValue(), '');
  assert.equal(await clear.isVisible(), false);
  assert.equal(await page.locator('#searchResults button').count(), 0);
  assert.equal(await field.evaluate(el => el === document.activeElement), true);
  await page.keyboard.type('Record 21');
  assert.equal(await page.locator('#searchResults button').count(), 1);
  await clear.press('Enter');
  assert.equal(await field.inputValue(), '');
  assert.equal(await page.evaluate(() => state.view), 'library');
});

test('the fullscreen exit is revealed at the top edge for the mouse', async () => {
  await page.evaluate(() => {
    window.testFullscreen = false;
    playerWindow.isFullscreen = async () => window.testFullscreen;
    playerWindow.setFullscreen = async value => { window.testFullscreen = value; };
  });
  const exit = page.locator('#exitFullscreen');
  await page.mouse.move(100, 2);
  assert.equal(await exit.isVisible(), false);
  await page.locator('#menuBtn').click();
  await page.getByRole('group', {name:'Playback & display'}).getByText('Enter fullscreen').click();
  await page.mouse.move(100, 2);
  await exit.waitFor();
  await page.mouse.move(100, 150);
  await exit.waitFor({state:'hidden'});
  await page.touchscreen.tap(100, 3);
  assert.equal(await exit.isVisible(), false);
  await page.mouse.move(100, 2);
  await exit.click();
  await exit.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(() => window.testFullscreen), false);
});
