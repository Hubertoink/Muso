function createAlbumCrates({ invoke, focused, lookup, play, toast }) {
  const el = id => document.getElementById(id);
  const dock = el('crateDock'), dialog = el('cratesDialog');
  let data = { active: null, lists: [] };
  let ghost = null, draggedAlbum = null, confirmDelete = false, fromSettings = false;
  let pending = Promise.resolve();
  const selected = () => data.lists.find(c => c.id === data.active);
  function update(change) {
    pending = pending.then(async () => {
      data = await invoke('library_crates', { change });
      confirmDelete = false;
      render();
    }).catch(e => toast(String(e), 4500));
    return pending;
  }
  function rowButton(label, glyph, action, disabled) {
    const b = document.createElement('button');
    b.textContent = glyph; b.title = label; b.setAttribute('aria-label', label);
    b.disabled = disabled;
    b.addEventListener('click', action);
    return b;
  }
  function render() {
    const c = selected();
    dock.classList.toggle('hidden', !c && !ghost);
    el('dockName').textContent = c?.name || 'Crate 1';
    el('dockCount').textContent = c?.albums.length ?? 0;
    if (dialog.classList.contains('hidden')) return;
    el('cratePicker').replaceChildren(...data.lists.map(list => new Option(list.name, list.id, false, list.id === data.active)));
    el('cratePicker').disabled = !c;
    el('crateEditor').classList.toggle('hidden', !c);
    if (!c) return;
    el('crateName').value = c.name;
    el('deleteCrate').textContent = confirmDelete ? 'Confirm deletion' : 'Delete crate';
    el('playCrate').disabled = !c.albums.some(entry => lookup(entry.id));
    const album = focused();
    el('crateAddFocused').disabled = !album || c.albums.some(entry => entry.id === album.id);
    el('crateAlbums').replaceChildren(...c.albums.map((entry, i) => {
      const a = lookup(entry.id), li = document.createElement('li');
      const name = document.createElement('span');
      name.textContent = (a || entry).title + ' — ' + (a || entry).artist + (a ? '' : ' (unavailable)');
      const edit = (type, direction) => update({type, id:c.id, album:entry.id, direction});
      li.append(name, rowButton('Move up', '↑', () => edit('move', -1), i === 0),
        rowButton('Move down', '↓', () => edit('move', 1), i === c.albums.length - 1),
        rowButton('Remove album', '×', () => edit('remove'), false));
      return li;
    }));
  }
  function open(settings) {
    fromSettings = settings;
    el('settings').classList.add('hidden');
    dialog.classList.remove('hidden');
    confirmDelete = false;
    render();
    update(null);
    el('newCrate').focus();
  }
  function close() {
    dialog.classList.add('hidden');
    if (fromSettings) {
      el('settings').classList.remove('hidden');
      el('manageCrates').focus();
    } else dock.focus();
  }
  dock.addEventListener('click', () => open(false));
  el('manageCrates').addEventListener('click', () => open(true));
  dialog.querySelector('.close').addEventListener('click', close);
  dialog.addEventListener('click', e => { if (e.target === dialog) close(); });
  dialog.addEventListener('keydown', e => {
    if (e.key === 'Escape') { e.stopPropagation(); close(); }
  });
  el('newCrate').addEventListener('click', () => update({type:'create'}));
  el('cratePicker').addEventListener('change', e => update({type:'select', id:Number(e.target.value)}));
  el('crateRename').addEventListener('submit', e => {
    e.preventDefault();
    if (selected()) update({type:'rename', id:data.active, name:el('crateName').value});
  });
  el('crateAddFocused').addEventListener('click', () => {
    if (focused()) update({type:'add', id:data.active, album:focused().id});
  });
  el('deleteCrate').addEventListener('click', () => {
    if (confirmDelete) update({type:'delete', id:data.active});
    else { confirmDelete = true; render(); }
  });
  el('playCrate').addEventListener('click', () => {
    const albums = selected()?.albums.map(entry => lookup(entry.id)).filter(Boolean);
    if (!albums?.length) return;
    dialog.classList.add('hidden');
    play(albums);
  });
  function drag(x, y) {
    ghost.style.left = x + 'px';
    ghost.style.top = y + 'px';
    dock.classList.toggle('over', dock.contains(document.elementFromPoint(x, y)));
  }
  function begin(album, sleeve, x, y) {
    draggedAlbum = album;
    ghost = document.createElement('div');
    ghost.className = 'crate-ghost';
    const image = sleeve.querySelector('img');
    if (image) ghost.appendChild(image.cloneNode());
    else ghost.textContent = album.title;
    document.body.appendChild(ghost);
    dock.classList.add('receiving');
    render();
    drag(x, y);
  }
  function end(x, y, cancelled) {
    const drop = !cancelled && dock.contains(document.elementFromPoint(x, y));
    const id = draggedAlbum.id;
    ghost.remove(); ghost = null; draggedAlbum = null;
    dock.classList.remove('receiving', 'over');
    if (drop) {
      if (selected()?.albums.some(entry => entry.id === id)) toast('Already in ' + selected().name);
      else update({type:'add', id:data.active, album:id});
    }
    render();
  }
  return { load: () => update(null), begin, drag, end };
}
