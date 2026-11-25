'use strict';

function createHeosOutput(invoke, listen, toast, outputChanged) {
  const el = id => document.getElementById(id);
  const dialog = el('audioDialog'), status = el('heosStatus');
  let rooms = {players: [], groups: [], output: null};
  let editor = null, busy = false;
  const room = id => rooms.players.find(r => r.pid === id);
  const groupOf = id => rooms.groups.find(g => g.players.some(p => p.pid === id));
  const leaderOf = g => g.players.find(p => p.role.toLowerCase() === 'leader')?.pid ?? g.gid;
  const freeRooms = () => rooms.players.filter(r => !groupOf(r.pid));
  const names = g => g.players.map(p => room(p.pid)?.name ?? String(p.pid)).join(' · ');
  el('heosIp').value = localStorage.getItem('muso.heosAddress') || '';

  async function run(message, work) {
    if (busy) return;
    busy = true;
    el('outputChoices').inert = el('groupEditor').inert = true;
    status.textContent = message;
    try { await work(); }
    catch (e) { status.textContent = String(e); }
    finally {
      busy = false;
      el('outputChoices').inert = el('groupEditor').inert = false;
    }
  }
  async function refresh(address = null) {
    await run('Looking for HEOS speakers…', async () => {
      rooms = await invoke('heos_rooms', { address, hint: localStorage.getItem('muso.heosAddress') });
      render();
      status.textContent = rooms.players.length ? '' : 'No HEOS rooms found. Check that the speaker and this computer are on the same network.';
      if (rooms.host && rooms.players.length) {
        localStorage.setItem('muso.heosAddress', rooms.host);
        el('heosIp').value = rooms.host;
        el('heosManual').open = false;
      }
    });
  }
  function select(pid, name) {
    return run('Switching audio output…', async () => {
      await invoke('heos_output', {pid});
      rooms.output = pid;
      render();
      status.textContent = 'Audio output · ' + name;
    });
  }
  function outputRow(pid, name, detail, active, group) {
    const row = document.createElement('div');
    row.className = 'heos-row';
    const button = document.createElement('button');
    button.className = 'heos-target';
    button.setAttribute('aria-pressed', String(active));
    button.onclick = () => select(pid, name);
    const text = document.createElement('span'), title = document.createElement('strong');
    title.textContent = name;
    text.append(title);
    if (detail) {
      const sub = document.createElement('small');
      sub.textContent = detail;
      text.append(sub);
    }
    button.append(text);
    row.append(button);
    if (group) {
      const edit = document.createElement('button');
      edit.className = 'link heos-edit';
      edit.textContent = 'Edit group';
      edit.setAttribute('aria-label', 'Edit group ' + name);
      edit.onclick = () => beginGroup(group);
      row.append(edit);
    }
    return row;
  }
  let lastOutput = null;
  function render() {
    if (rooms.output !== lastOutput) { lastOutput = rooms.output; outputChanged(); }
    el('albumHeos').classList.toggle('hidden', rooms.output == null);
    el('localOutput').replaceChildren(outputRow(null, 'This computer', 'System audio output', rooms.output === null));
    el('heosRooms').replaceChildren(
      ...rooms.groups.map(g => outputRow(leaderOf(g), g.name, names(g), groupOf(rooms.output) === g, g)),
      ...freeRooms().map(r => outputRow(r.pid, r.name, r.model, rooms.output === r.pid))
    );
    el('heosCreate').hidden = freeRooms().length < 2;
  }
  function beginGroup(group) {
    const available = rooms.players.filter(r => !groupOf(r.pid) || groupOf(r.pid) === group);
    const leader = group ? leaderOf(group) : (available.find(r => r.pid === rooms.output) ?? available[0]).pid;
    const previous = group?.players.map(p => p.pid) ?? [];
    editor = {leader, available, previous, selected:new Set(previous)};
    el('audioTitle').textContent = group ? 'Edit group' : 'Create group';
    el('heosDissolve').hidden = !group;
    const select = el('heosLeader');
    select.replaceChildren(...available.map(r => new Option(r.name, String(r.pid))));
    select.value = String(leader);
    select.disabled = !!group;
    el('outputChoices').hidden = true;
    el('groupEditor').hidden = false;
    status.textContent = '';
    renderMembers();
    (group ? el('heosCancel') : select).focus();
  }
  function renderMembers() {
    el('heosMembers').replaceChildren(...editor.available.filter(r => r.pid !== editor.leader).map(r => {
      const label = document.createElement('label');
      const check = document.createElement('input');
      check.type = 'checkbox';
      check.checked = editor.selected.has(r.pid);
      check.onchange = () => {
        if (check.checked) editor.selected.add(r.pid); else editor.selected.delete(r.pid);
        updateSave();
      };
      label.append(check, document.createTextNode(r.name));
      return label;
    }));
    updateSave();
  }
  function members() { return [editor.leader, ...[...editor.selected].filter(p => p !== editor.leader)]; }
  function updateSave() {
    const list = members();
    el('heosSave').disabled = list.length < 2 || (list.length === editor.previous.length && list.every(p => editor.previous.includes(p)));
  }
  el('heosLeader').onchange = () => {
    editor.leader = Number(el('heosLeader').value);
    renderMembers();
  };
  function endGroup() {
    editor = null;
    el('audioTitle').textContent = 'Audio output';
    el('groupEditor').hidden = true;
    el('outputChoices').hidden = false;
    render();
  }
  function save(dissolve) {
    const list = dissolve ? [editor.leader] : members();
    return run('Updating HEOS group…', async () => {
      rooms = await invoke('heos_group', {members:list, previous:editor.previous});
      endGroup();
      status.textContent = dissolve ? 'Rooms are separate again.' : 'Group saved.';
    });
  }
  el('heosSave').onclick = () => save(false);
  el('heosDissolve').onclick = () => save(true);
  el('heosCancel').onclick = () => { endGroup(); status.textContent = ''; };
  el('heosCreate').onclick = () => beginGroup(null);
  el('audioClose').onclick = () => dialog.close();
  el('heosRefresh').onclick = () => refresh();
  el('heosConnect').onsubmit = e => { e.preventDefault(); refresh(el('heosIp').value.trim()); };
  const volume = el('heosVolumeDialog'), levels = el('heosLevels'), volumeStatus = el('heosVolumeStatus');
  let visit = 0, writes = Promise.resolve();
  function writeLevel(args) {
    const result = writes.then(() => invoke('heos_set_level', args));
    writes = result.catch(() => {});
    return result;
  }
  async function readLevels() {
    await writes;
    return invoke('heos_levels');
  }
  async function openVolume() {
    const current = ++visit;
    levels.replaceChildren();
    volumeStatus.textContent = 'Reading speaker volume…';
    volume.showModal();
    try {
      const snapshot = await readLevels();
      if (current !== visit || !volume.open) return;
      volumeStatus.textContent = '';
      let revision = 0;
      const sliders = new Map();
      const key = c => (c.group ? 'group-' : 'room-') + c.id;
      function reflect(snapshot) {
        for (const c of snapshot.controls) {
          const slider = sliders.get(key(c));
          if (slider) {
            slider.input.value = c.value;
            slider.output.value = c.value;
            slider.capsule.style.setProperty('--volume-factor', c.value / 100);
          }
        }
      }
      for (const c of snapshot.controls) {
        const label = document.createElement('label'), title = document.createElement('span');
        const input = document.createElement('input'), output = document.createElement('output');
        label.className = 'heos-level';
        title.textContent = c.group ? 'Group · ' + c.name : c.name;
        input.type = 'range';
        input.max = '100';
        input.min = '0';
        input.setAttribute('aria-orientation', 'vertical');
        input.setAttribute('aria-label', title.textContent);
        input.id = 'volume-' + key(c);
        output.htmlFor.add(input.id);
        label.htmlFor = input.id;
        const capsule = document.createElement('span');
        capsule.className = 'volume-capsule';
        capsule.append(input);
        sliders.set(key(c), {input, output, capsule});
        input.oninput = () => {
          ++revision;
          output.value = input.value;
          capsule.style.setProperty('--volume-factor', input.valueAsNumber / 100);
        };
        input.onchange = async () => {
          const value = Number(input.value), edit = ++revision;
          try {
            const updated = await writeLevel({session:snapshot.session, group:c.group, id:c.id, value});
            if (visit === current && edit === revision) { reflect(updated); volumeStatus.textContent = ''; }
          } catch (e) {
            if (visit !== current || edit !== revision) return;
            volumeStatus.textContent = String(e);
            try {
              const actual = await readLevels();
              if (visit === current && edit === revision) reflect(actual);
            } catch { if (visit === current && edit === revision) levels.replaceChildren(); }
          }
        };
        label.append(title, output, capsule);
        levels.append(label);
      }
      reflect(snapshot);
    } catch (e) { if (visit === current) volumeStatus.textContent = String(e); }
  }
  el('heosVolumeClose').onclick = () => volume.close();
  volume.addEventListener('close', () => { ++visit; });
  listen('heos-control-error', ({payload}) => toast(String(payload), 4500));
  listen('heos-disconnected', () => {
    rooms.output = null;
    if (volume.open) volume.close();
    render();
    toast('HEOS disconnected · Audio returned to this computer');
  });
  return {openVolume, readLevels, writeLevel, get output() { return rooms.output; }, open() {
    endGroup();
    dialog.showModal();
    refresh();
  }};
}
