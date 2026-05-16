const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const tauriWin = () => window.__TAURI__.window.getCurrentWindow();
const $ = (s, r=document) => r.querySelector(s);

let allApps = [];
let activeCategory = 'all';
let theme = 'auto';
let pmInfo = { pm: 'pacman', exts: ['.pkg.tar.zst'] };
const iconCache = new Map();

// Disable right-click globally
window.addEventListener('contextmenu', e => e.preventDefault());

async function getIcon(name, repo) {
  if (iconCache.has(name)) return iconCache.get(name);
  // 1) Local hicolor (app installed → real system icon)
  try {
    const d = await invoke('get_icon', { name });
    if (d) { iconCache.set(name, d); return d; }
  } catch {}
  // 2) Fetch from app's GitHub repo (pre-install)
  if (repo) {
    const url = `https://raw.githubusercontent.com/${repo}/main/src-tauri/icons/icon.png`;
    try {
      const r = await fetch(url, { cache: 'force-cache' });
      if (r.ok) {
        const blob = await r.blob();
        const data = await new Promise(res => { const fr = new FileReader(); fr.onload = () => res(fr.result); fr.readAsDataURL(blob); });
        iconCache.set(name, data);
        return data;
      }
    } catch {}
  }
  iconCache.set(name, '');
  return '';
}

function iconHtml(app, cls) {
  const data = iconCache.get(app.icon || app.pkg);
  if (data) return `<img class="${cls}-img" src="${data}" alt="" draggable="false">`;
  return `<span>${escapeHtml((app.label||'?')[0])}</span>`;
}

// ───── THEME ─────
async function applyTheme() {
  const root = document.documentElement;
  root.classList.remove('light-mode','dark-mode');
  if (theme === 'light') root.classList.add('light-mode');
  else if (theme === 'dark') root.classList.add('dark-mode');
  else {
    try {
      const sys = await invoke('detect_system_theme');
      if (sys === 'dark') root.classList.add('dark-mode');
      else if (sys === 'light') root.classList.add('light-mode');
    } catch {}
  }
}
function cycleTheme(){theme = theme==='auto'?'light':theme==='light'?'dark':'auto';applyTheme();toast('Tema: '+(theme==='auto'?'Auto':theme==='light'?'Claro':'Oscuro'));}

// ───── TOAST ─────
let toastTimer;
function toast(msg){const t=$('#toast');t.textContent=msg;t.classList.add('show');clearTimeout(toastTimer);toastTimer=setTimeout(()=>t.classList.remove('show'),2200);}

// ───── LOAD APPS ─────
async function loadApps() {
  try { pmInfo = await invoke('pm_info'); } catch {}
  try {
    allApps = await invoke('list_apps');
    console.log('catalog loaded:', allApps.length, 'apps');
  } catch (e) {
    console.error('list_apps failed:', e);
    toast('Error catálogo: ' + (typeof e==='string'?e:e.message||JSON.stringify(e)).slice(0,140));
    allApps = [];
    renderCategories();
    renderGrid();
    return;
  }
  await Promise.all(allApps.map(a => getIcon(a.icon || a.pkg, a.repo)));
  renderCategories();
  renderGrid();
  for (const a of allApps) {
    fetchReleaseInfo(a).catch(err => console.warn('fetch', a.repo, 'failed:', err));
  }
}

async function fetchReleaseInfo(a) {
  if (!a.repo) { a.available = null; return; }
  try {
    const r = await fetch(`https://api.github.com/repos/${a.repo}/releases/latest`, {
      headers: { 'Accept': 'application/vnd.github+json' },
      cache: 'no-cache'
    });
    if (!r.ok) { a.available = null; a.has_update = false; renderGrid(); return; }
    const d = await r.json();
    const tag = (d.tag_name || '').replace(/^v/i,'').trim();
    a.available = tag;
    a.release_assets = d.assets || [];
    a.release_url = d.html_url;
    if (a.installed) {
      const cur = (a.installed||'').split('-')[0];
      a.has_update = !!(tag && cmpVer(tag, cur) > 0);
    } else {
      a.has_update = false;
    }
    renderGrid();
  } catch (e) {
    console.warn('release fetch failed', a.repo, e);
    a.available = null;
    a.has_update = false;
  }
}

function cmpVer(a,b){
  const pa=a.split(/[.\-+]/).map(s=>parseInt(s,10)||0);
  const pb=b.split(/[.\-+]/).map(s=>parseInt(s,10)||0);
  for (let i=0;i<Math.max(pa.length,pb.length);i++){const x=pa[i]||0,y=pb[i]||0;if(x!==y) return x-y;}
  return 0;
}

// ───── CATEGORIES ─────
function renderCategories() {
  const cats = ['all', ...new Set(allApps.map(a => a.category))];
  const labels = { all: 'Todas' };
  const c = $('#categories');
  c.innerHTML = '';
  for (const cat of cats) {
    const b = document.createElement('button');
    b.className = 'cat-pill' + (cat === activeCategory ? ' on' : '');
    b.textContent = labels[cat] || cat;
    b.addEventListener('click', () => { activeCategory = cat; renderCategories(); renderGrid(); });
    c.appendChild(b);
  }
}

// ───── GRID ─────
function renderGrid() {
  const q = ($('#search')?.value || '').toLowerCase().trim();
  let list = allApps;
  if (activeCategory !== 'all') list = list.filter(a => a.category === activeCategory);
  if (q) list = list.filter(a =>
    a.label.toLowerCase().includes(q) ||
    a.pkg.toLowerCase().includes(q) ||
    (a.description||'').toLowerCase().includes(q)
  );
  refreshUpdateAllBadge();
  const grid = $('#apps-grid');
  const empty = $('#empty');
  grid.innerHTML = '';
  if (list.length === 0) { empty.classList.remove('hidden'); return; }
  empty.classList.add('hidden');
  for (const a of list) {
    const card = document.createElement('div');
    card.className = 'app-card';
    card.style.setProperty('--accent', a.accent || '#0a84ff');
    const status = a.has_update
      ? `<span class="app-status update"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>Actualizar</span>`
      : a.installed
        ? `<span class="app-status installed"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>Instalada</span>`
        : `<span class="app-status"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/></svg>Instalar</span>`;
    card.innerHTML = `
      <div class="accent"></div>
      <div class="app-icon">${iconHtml(a, 'app-icon')}</div>
      <div class="app-title">${escapeHtml(a.label)}</div>
      <div class="app-desc">${escapeHtml(a.description || '')}</div>
      <div class="app-foot">
        ${status}
        <span class="app-cat">${escapeHtml(a.category)}</span>
      </div>`;
    card.addEventListener('click', () => openDrawer(a));
    grid.appendChild(card);
  }
}

function escapeHtml(s){return (s||'').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}

function refreshUpdateAllBadge() {
  const n = allApps.filter(a => a.has_update).length;
  const btn = $('#update-all-btn');
  const c = $('#update-all-count');
  if (!btn) return;
  if (n > 0) { btn.classList.remove('hidden'); c.textContent = n; }
  else btn.classList.add('hidden');
}

// ───── DRAWER ─────
function openDrawer(app) {
  const d = $('#drawer');
  d.classList.remove('hidden');
  const card = $('#drawer-card');
  card.style.setProperty('--accent', app.accent || '#0a84ff');
  $('#d-icon').innerHTML = iconHtml(app, 'drawer-icon');
  $('#d-title').textContent = app.label;
  $('#d-pkg').textContent = app.pkg;
  const ver = app.installed
    ? `Instalada: ${app.installed.split('-')[0]}${app.available ? ' · Última: '+app.available : ''}`
    : (app.available ? `Última versión: ${app.available}` : 'No instalada');
  $('#d-version').textContent = ver;
  $('#d-desc').textContent = app.description || '';
  $('#d-repo').href = `https://github.com/${app.repo}`;
  renderDrawerActions(app);
  $('#d-progress').classList.add('hidden');
}

function renderDrawerActions(app) {
  const a = $('#d-actions');
  a.innerHTML = '';
  if (!app.installed) {
    addBtn(a, 'Instalar', 'primary', () => doInstall(app));
  } else if (app.has_update) {
    addBtn(a, 'Actualizar', 'primary', () => doInstall(app, true));
    if (!app.self) addBtn(a, 'Abrir', '', () => doLaunch(app));
    if (!app.self) addBtn(a, 'Desinstalar', 'danger', () => doUninstall(app));
  } else {
    if (app.self) {
      addBtn(a, 'Estás en la última versión', '', () => {}).disabled = true;
    } else {
      addBtn(a, 'Abrir', 'primary', () => doLaunch(app));
      addBtn(a, 'Desinstalar', 'danger', () => doUninstall(app));
    }
  }
}

function addBtn(parent, label, cls, fn) {
  const b = document.createElement('button');
  b.className = 'drawer-btn ' + cls;
  b.textContent = label;
  b.addEventListener('click', fn);
  parent.appendChild(b);
  return b;
}

function closeDrawer(){ $('#drawer').classList.add('hidden'); }

// ───── ACTIONS ─────
async function doLaunch(app) {
  try { await invoke('launch_app', { pkg: app.pkg }); toast('Lanzando '+app.label+'…'); closeDrawer(); }
  catch (e) { toast('Error: ' + e); }
}

let currentOpId = null;
let progressTimer = null;
function newOpId() { return 'op-' + Math.random().toString(36).slice(2,10) + '-' + Date.now(); }

function startProgressPoll(opId) {
  stopProgressPoll();
  progressTimer = setInterval(async () => {
    try {
      const p = await invoke('progress', { opId });
      if (!p || !p.active) { stopProgressPoll(); return; }
      const pct = Math.max(0, Math.min(100, p.pct || 0));
      const fill = $('#d-progress-fill');
      if (fill) { fill.style.width = pct.toFixed(1)+'%'; fill.style.animation = 'none'; }
      const txt = $('#d-progress-text');
      if (txt && p.total > 0) {
        txt.textContent = `Descargando… ${pct.toFixed(0)}% · ${fmtBytes(p.downloaded)} / ${fmtBytes(p.total)}`;
      }
    } catch {}
  }, 250);
}
function stopProgressPoll() { if (progressTimer) { clearInterval(progressTimer); progressTimer = null; } }
function fmtBytes(b) {
  if (!b) return '0 B';
  const u = ['B','KB','MB','GB']; let i = 0;
  while (b >= 1024 && i < u.length-1) { b /= 1024; i++; }
  return b.toFixed(b<10?1:0) + ' ' + u[i];
}

async function doInstall(app, isUpdate=false) {
  const exts = pmInfo.exts || ['.pkg.tar.zst'];
  const asset = (app.release_assets || []).find(a => exts.some(e => a.name.endsWith(e)));
  if (!asset) {
    if (!confirm(`No hay paquete ${exts.join('/')} en la release. ¿Abrir página de GitHub?`)) return;
    invoke('open_release_page', { repo: app.repo });
    closeDrawer();
    return;
  }
  const pwd = await promptAuth(
    isUpdate ? 'Actualizar app' : 'Instalar app',
    `Para ${isUpdate?'actualizar':'instalar'} "${app.label}", introduce la contraseña del equipo.`);
  if (pwd === null) return;
  const opId = newOpId();
  currentOpId = opId;
  setProgress(true, 'Descargando '+asset.name+'…', true);
  startProgressPoll(opId);
  try {
    const localPath = await invoke('download_pkg', { url: asset.browser_download_url, filename: asset.name, opId });
    stopProgressPoll();
    setProgress(true, 'Instalando…', false);
    await invoke('install_pkg_file', { path: localPath, password: pwd, opId });
    currentOpId = null;
    if (app.self && isUpdate) {
      toast('Bookos Store actualizada. Reiniciando…');
      setTimeout(() => tauriWin().close(), 1400);
      return;
    }
    toast((isUpdate?'Actualizada: ':'Instalada: ') + app.label);
    closeDrawer();
    await loadApps();
  } catch (e) {
    stopProgressPoll();
    currentOpId = null;
    const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
    if (msg.includes('__cancelled__')) toast('Operación cancelada');
    else toast('Error: ' + msg.slice(0,140));
    setProgress(false);
  }
}

async function doUninstall(app) {
  const pwd = await promptAuth('Desinstalar app',
    `Para desinstalar "${app.label}" (${app.pkg}), introduce la contraseña del equipo.`);
  if (pwd === null) return;
  const opId = newOpId();
  currentOpId = opId;
  setProgress(true, 'Desinstalando…', true);
  try {
    await invoke('uninstall_pkg', { pkg: app.pkg, password: pwd, opId });
    currentOpId = null;
    toast('Desinstalada: ' + app.label);
    closeDrawer();
    await loadApps();
  } catch (e) {
    currentOpId = null;
    const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
    if (msg.includes('__cancelled__')) toast('Operación cancelada');
    else toast('Error: ' + msg.slice(0,140));
    setProgress(false);
  }
}

async function cancelCurrentOp() {
  if (!currentOpId) return;
  await invoke('cancel_op', { opId: currentOpId });
}

async function doUpdateAll() {
  const list = allApps.filter(a => a.has_update);
  if (list.length === 0) return;
  const pwd = await promptAuth('Actualizar todas',
    `Se actualizarán ${list.length} app${list.length>1?'s':''}. Introduce la contraseña del equipo.`);
  if (pwd === null) return;
  let needsRestart = false;
  for (const app of list) {
    const exts = pmInfo.exts || ['.pkg.tar.zst'];
    const asset = (app.release_assets || []).find(a => exts.some(e => a.name.endsWith(e)));
    if (!asset) { toast('Saltada (sin paquete): '+app.label); continue; }
    const opId = newOpId();
    currentOpId = opId;
    openDrawer(app);
    setProgress(true, 'Descargando '+asset.name+'…', true);
    startProgressPoll(opId);
    try {
      const localPath = await invoke('download_pkg', { url: asset.browser_download_url, filename: asset.name, opId });
      stopProgressPoll();
      setProgress(true, 'Instalando '+app.label+'…', false);
      await invoke('install_pkg_file', { path: localPath, password: pwd, opId });
      toast('Actualizada: '+app.label);
      if (app.self) needsRestart = true;
    } catch (e) {
      stopProgressPoll();
      const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
      if (msg.includes('__cancelled__')) { toast('Cancelado'); break; }
      toast('Error en '+app.label+': '+msg.slice(0,100));
    }
  }
  currentOpId = null;
  setProgress(false);
  closeDrawer();
  if (needsRestart) {
    toast('Bookos Store actualizada. Reiniciando…');
    setTimeout(() => tauriWin().close(), 1400);
    return;
  }
  await loadApps();
}

// ───── AUTH DIALOG ─────
function promptAuth(title, desc) {
  return new Promise(resolve => {
    const ov = $('#auth-overlay');
    const input = $('#auth-input');
    const err = $('#auth-err');
    $('#auth-title').textContent = title;
    $('#auth-desc').textContent = desc;
    input.value = ''; input.type = 'password';
    err.classList.add('hidden');
    ov.classList.remove('hidden');
    setTimeout(() => input.focus(), 60);

    const cleanup = (val) => {
      ov.classList.add('hidden');
      okBtn.removeEventListener('click', onOk);
      cancelBtn.removeEventListener('click', onCancel);
      input.removeEventListener('keydown', onKey);
      eyeBtn.removeEventListener('click', onEye);
      ov.removeEventListener('click', onBg);
      resolve(val);
    };
    const onOk = async () => {
      const p = input.value;
      if (!p) { err.textContent='Contraseña vacía'; err.classList.remove('hidden'); return; }
      okBtn.disabled = true;
      const ok = await invoke('verify_password', { password: p });
      okBtn.disabled = false;
      if (!ok) { err.textContent='Contraseña incorrecta'; err.classList.remove('hidden'); input.select(); return; }
      cleanup(p);
    };
    const onCancel = () => cleanup(null);
    const onKey = (e) => { if (e.key==='Enter') onOk(); else if (e.key==='Escape') onCancel(); };
    const onEye = () => { input.type = input.type==='password'?'text':'password'; };
    const onBg = (e) => { if (e.target.id==='auth-overlay') onCancel(); };
    const okBtn = $('#auth-ok'), cancelBtn = $('#auth-cancel'), eyeBtn = $('#auth-eye');
    okBtn.addEventListener('click', onOk);
    cancelBtn.addEventListener('click', onCancel);
    input.addEventListener('keydown', onKey);
    eyeBtn.addEventListener('click', onEye);
    ov.addEventListener('click', onBg);
  });
}

function setProgress(on, text, cancellable=false) {
  const p = $('#d-progress');
  p.classList.toggle('hidden', !on);
  if (text) $('#d-progress-text').textContent = text;
  const fill = $('#d-progress-fill');
  if (fill) {
    if (cancellable) { fill.style.animation = 'none'; fill.style.width = '0%'; }
    else { fill.style.animation = ''; fill.style.width = ''; }
  }
  document.querySelectorAll('#d-actions .drawer-btn').forEach(b => b.disabled = on);
  const cBtn = $('#d-cancel-btn');
  if (cBtn) cBtn.classList.toggle('hidden', !(on && cancellable));
}

// ───── WIRE ─────
function wire() {
  $('#minimize').addEventListener('click', () => tauriWin().minimize());
  $('#maximize').addEventListener('click', () => tauriWin().toggleMaximize());
  $('#close').addEventListener('click', () => tauriWin().close());
  $('#theme-btn').addEventListener('click', cycleTheme);
  $('#refresh-btn').addEventListener('click', () => loadApps());
  $('#update-all-btn').addEventListener('click', () => doUpdateAll());
  $('#d-cancel-btn').addEventListener('click', () => cancelCurrentOp());
  $('#search').addEventListener('input', () => { renderGrid(); });
  $('#drawer-close').addEventListener('click', closeDrawer);
  $('#drawer').addEventListener('click', (e) => { if (e.target.id === 'drawer') closeDrawer(); });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeDrawer();
    if ((e.ctrlKey||e.metaKey) && e.key === 'f') { e.preventDefault(); $('#search').focus(); }
  });
}

(async function init(){
  await applyTheme();
  wire();
  await loadApps();
})();
