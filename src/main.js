const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const tauriWin = () => window.__TAURI__.window.getCurrentWindow();
const $ = (s, r=document) => r.querySelector(s);

let allApps = [];
let activeCategory = 'all';
let theme = 'auto';
let pmInfo = { pm: 'pacman', exts: ['.pkg.tar.zst'] };
let currentApp = null;        // app object when in detail view, else null
let currentTab = 'overview';  // overview | screenshots | history
let lightboxIdx = -1;         // open screenshot index, -1 closed

// ───── i18n ─────
const i18n = (k, v) => (window.BookosI18n ? window.BookosI18n.t(k, v) : k);
const lang = () => (window.BookosI18n ? BookosI18n.getLang() : 'es');
let uiLang = localStorage.getItem('bookos.store.lang') || 'auto';
function setUiLang(l) {
  uiLang = l;
  localStorage.setItem('bookos.store.lang', l);
  if (window.BookosI18n) BookosI18n.setLang(l);
  toast(i18n('langToast', { l: i18n('lang.' + (uiLang === 'auto' ? 'auto' : uiLang)) }));
  if (allApps.length) { renderCategories(); render(); }
}

// Pick localized string from app: try `<field>_<lang>`, fall back to base field.
function pickL(obj, field) {
  const l = lang();
  if (l !== 'es') {
    const v = obj[field + '_' + l];
    if (v) return v;
  }
  return obj[field] || '';
}
const getLabel = a => pickL(a, 'label') || a.pkg;
const getDesc  = a => pickL(a, 'description');
const getNotes = r => pickL(r || {}, 'notes');

function assetCompat(asset, pm) {
  if (!asset || !asset.name) return false;
  const k = (asset.kind || '').toLowerCase();
  if (k) {
    if (pm === 'pacman' && k === 'pacman') return true;
    if (pm === 'dnf'    && (k === 'dnf' || k === 'rpm')) return true;
    if (pm === 'apt'    && (k === 'apt' || k === 'deb')) return true;
    if (k === 'appimage') return true;
    return false;
  }
  const n = asset.name.toLowerCase();
  if (pm === 'pacman') return n.endsWith('.pkg.tar.zst');
  if (pm === 'dnf')    return n.endsWith('.rpm');
  if (pm === 'apt')    return n.endsWith('.deb');
  return false;
}

const iconCache = new Map();
window.addEventListener('contextmenu', e => e.preventDefault());

async function getIcon(name, repo, iconUrl) {
  if (iconCache.has(name)) return iconCache.get(name);
  try {
    const d = await invoke('get_icon', { name });
    if (d) { iconCache.set(name, d); return d; }
  } catch {}
  if (iconUrl) {
    try {
      const r = await fetch(iconUrl, { cache: 'no-store' });
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
  return `<span>${escapeHtml((getLabel(app)||'?')[0])}</span>`;
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
function cycleTheme(){theme = theme==='auto'?'light':theme==='light'?'dark':'auto';applyTheme();toast(i18n('themeToast', { t: i18n('theme.' + theme) }));}
function cycleLang(){const order=['auto','es','en'];const i=order.indexOf(uiLang);setUiLang(order[(i+1)%order.length]);}

// ───── TOAST ─────
let toastTimer;
function toast(msg){const t=$('#toast');t.textContent=msg;t.classList.add('show');clearTimeout(toastTimer);toastTimer=setTimeout(()=>t.classList.remove('show'),2200);}

// ───── LOAD APPS ─────
async function loadApps() {
  try { pmInfo = await invoke('pm_info'); } catch {}
  try {
    allApps = await invoke('list_apps');
  } catch (e) {
    toast('Error catálogo: ' + (typeof e==='string'?e:e.message||JSON.stringify(e)).slice(0,140));
    allApps = [];
    renderCategories();
    render();
    return;
  }
  for (const a of allApps) {
    a.release_assets = a.assets || [];
    a.release_url    = a.html_url || '';
    if (a.installed && a.available) {
      const cur = String(a.installed).split('-')[0];
      a.has_update = cmpVer(a.available, cur) > 0;
    } else {
      a.has_update = false;
    }
  }
  await Promise.all(allApps.map(a => getIcon(a.icon || a.pkg, a.pkg, a.icon_url)));
  renderCategories();
  render();
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
  const labels = { all: i18n('all') };
  const c = $('#categories');
  c.innerHTML = '';
  for (const cat of cats) {
    const b = document.createElement('button');
    b.className = 'cat-pill' + (cat === activeCategory ? ' on' : '');
    b.textContent = labels[cat] || cat;
    b.addEventListener('click', () => { activeCategory = cat; renderCategories(); render(); });
    c.appendChild(b);
  }
}

// ───── DISPATCH ─────
function render() {
  if (currentApp) renderDetail();
  else renderGrid();
}

// ───── GRID ─────
function renderGrid() {
  $('#detail-view')?.classList.add('hidden');
  $('#grid-view')?.classList.remove('hidden');
  $('#categories')?.classList.remove('hidden');

  const q = ($('#search')?.value || '').toLowerCase().trim();
  let list = allApps;
  if (activeCategory !== 'all') list = list.filter(a => a.category === activeCategory);
  if (q) list = list.filter(a =>
    getLabel(a).toLowerCase().includes(q) ||
    a.pkg.toLowerCase().includes(q) ||
    getDesc(a).toLowerCase().includes(q)
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
    const pmName = pmInfo.pm || 'pacman';
    const hasCompat = (a.release_assets || []).some(x => assetCompat(x, pmName));
    const status = a.has_update
      ? `<span class="app-status update"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>${i18n('update')}</span>`
      : a.installed
        ? `<span class="app-status installed"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>${i18n('installed')}</span>`
        : hasCompat
          ? `<span class="app-status"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/></svg>${i18n('install')}</span>`
          : `<span class="app-status" style="opacity:.55">—</span>`;
    card.innerHTML = `
      <div class="accent"></div>
      <div class="app-icon">${iconHtml(a, 'app-icon')}</div>
      <div class="app-title">${escapeHtml(getLabel(a))}</div>
      <div class="app-desc">${escapeHtml(getDesc(a))}</div>
      <div class="app-foot">
        ${status}
        <span class="app-cat">${escapeHtml(a.category)}</span>
      </div>`;
    card.addEventListener('click', () => openDetail(a));
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

// ───── DETAIL VIEW ─────
async function openDetail(app) {
  currentApp = app;
  currentTab = 'overview';

  // Fetch full per-app detail (with releases history) from catalog endpoint.
  // Tauri client already cached catalog; here we hit the per-app endpoint
  // for the `releases[]` array which the grid response omits.
  try {
    const url = 'https://bookos.es/api/store.json?app=' + encodeURIComponent(app.pkg);
    const r = await fetch(url, { cache: 'no-store' });
    if (r.ok) {
      const full = await r.json();
      // Merge releases + any updated localized fields without losing local state.
      Object.assign(currentApp, full);
      currentApp.release_assets = full.assets || currentApp.release_assets || [];
    }
  } catch {}

  render();
}

function closeDetail() {
  currentApp = null;
  lightboxIdx = -1;
  $('#lightbox')?.classList.add('hidden');
  render();
}

function renderDetail() {
  $('#grid-view')?.classList.add('hidden');
  $('#categories')?.classList.add('hidden');
  $('#detail-view')?.classList.remove('hidden');

  const a = currentApp;
  const dv = $('#detail-view');
  const accent = a.accent || '#0a84ff';
  dv.style.setProperty('--accent', accent);

  const pmName = pmInfo.pm || 'pacman';
  const hasCompat = (a.release_assets || []).some(x => assetCompat(x, pmName));

  const verLine = a.installed
    ? i18n('detail.installedAt', { v: String(a.installed).split('-')[0] }) +
      (a.available && cmpVer(a.available, String(a.installed).split('-')[0]) > 0
        ? ' · ' + i18n('detail.latest', { v: a.available }) : '')
    : (a.available ? i18n('detail.latest', { v: a.available }) : i18n('detail.notInstalled'));

  // Action buttons
  let actionsHtml = '';
  if (!a.installed) {
    if (hasCompat) actionsHtml = `<button class="detail-action primary" data-act="install">${svgIcon('download')}${i18n('install')}</button>`;
    else actionsHtml = `<button class="detail-action" disabled title="${escapeHtml(i18n('detail.noCompat', { pm: pmName }))}">${i18n('detail.noPkg', { pm: pmName })}</button>`;
  } else {
    if (a.has_update && hasCompat) actionsHtml += `<button class="detail-action primary" data-act="update">${svgIcon('refresh')}${i18n('update')}</button>`;
    if (!a.self) actionsHtml += `<button class="detail-action" data-act="launch">${svgIcon('play')}${i18n('open')}</button>`;
    if (!a.self) actionsHtml += `<button class="detail-action danger" data-act="uninstall">${svgIcon('trash')}${i18n('uninstall')}</button>`;
    if (a.self && !a.has_update) actionsHtml += `<button class="detail-action" disabled>${svgIcon('check')}${i18n('detail.upToDate')}</button>`;
  }

  // Tabs (only show screenshots/history if data exists)
  const hasShots = (a.screenshots || []).length > 0;
  const hasHistory = (a.releases || []).length > 0;
  const tabsHtml = `
    <button class="tab ${currentTab==='overview'?'on':''}" data-tab="overview">${i18n('tab.overview')}</button>
    ${hasShots ? `<button class="tab ${currentTab==='screenshots'?'on':''}" data-tab="screenshots">${i18n('tab.screenshots')} <span class="tab-count">${a.screenshots.length}</span></button>` : ''}
    ${hasHistory ? `<button class="tab ${currentTab==='history'?'on':''}" data-tab="history">${i18n('tab.history')} <span class="tab-count">${a.releases.length}</span></button>` : ''}
  `;

  // Hero banner image (first screenshot) — fades behind icon, very nice visual
  const heroShot = hasShots ? a.screenshots[0] : '';

  dv.innerHTML = `
    <div class="detail-hero" style="${heroShot ? `--hero-bg:url('${escapeAttr(heroShot)}');` : ''}">
      <button class="detail-back" id="detail-back" title="${escapeHtml(i18n('back'))}">${svgIcon('back')}<span>${i18n('back')}</span></button>
      <div class="detail-hero-grad"></div>
    </div>
    <div class="detail-head">
      <div class="detail-icon">${iconHtml(a, 'detail-icon')}</div>
      <div class="detail-head-text">
        <h1 class="detail-title">${escapeHtml(getLabel(a))}</h1>
        <div class="detail-sub">
          <span class="detail-cat">${escapeHtml(a.category)}</span>
          ${a.author ? `<span>· ${escapeHtml(a.author)}</span>` : ''}
          ${a.license ? `<span>· ${escapeHtml(a.license)}</span>` : ''}
        </div>
        <div class="detail-pkg"><code>${escapeHtml(a.pkg)}</code></div>
        <div class="detail-ver">${escapeHtml(verLine)}</div>
      </div>
      <div class="detail-actions" id="detail-actions">${actionsHtml}</div>
    </div>
    <div class="detail-progress hidden" id="detail-progress">
      <div class="detail-progress-bar"><div class="detail-progress-fill" id="detail-progress-fill"></div></div>
      <div class="detail-progress-row">
        <div class="detail-progress-text" id="detail-progress-text">${i18n('downloading')}</div>
        <button class="detail-cancel hidden" id="detail-cancel">${i18n('cancel')}</button>
      </div>
    </div>
    <div class="tabs">${tabsHtml}</div>
    <div class="tab-body" id="tab-body"></div>
  `;

  // Wire actions
  dv.querySelector('#detail-back')?.addEventListener('click', closeDetail);
  dv.querySelectorAll('.detail-action').forEach(b => b.addEventListener('click', () => {
    const act = b.dataset.act;
    if (act === 'install')   doInstall(a, false);
    if (act === 'update')    doInstall(a, true);
    if (act === 'launch')    doLaunch(a);
    if (act === 'uninstall') doUninstall(a);
  }));
  dv.querySelectorAll('.tab').forEach(t => t.addEventListener('click', () => {
    currentTab = t.dataset.tab;
    renderDetail();
  }));

  renderTabBody();
}

function renderTabBody() {
  const a = currentApp;
  const body = $('#tab-body');
  if (!body) return;

  if (currentTab === 'overview') {
    const desc = getDesc(a) || i18n('detail.noDesc');
    const latest = (a.releases && a.releases[0]) || null;
    const notes = latest ? getNotes(latest) : '';
    body.innerHTML = `
      <section class="detail-section">
        <h2>${i18n('detail.about')}</h2>
        <p class="detail-desc">${formatMd(desc)}</p>
      </section>
      ${notes ? `
      <section class="detail-section">
        <h2>${i18n('detail.whatsNew')} <span class="muted">v${escapeHtml(latest.version)}${latest.released ? ' · ' + escapeHtml(latest.released) : ''}</span></h2>
        <div class="detail-notes">${formatMd(notes)}</div>
      </section>` : ''}
      ${a.html_url ? `<section class="detail-section"><a class="detail-link" href="${escapeAttr(a.html_url)}" target="_blank">${i18n('detail.openHomepage')}</a></section>` : ''}
    `;
  }
  else if (currentTab === 'screenshots') {
    const shots = a.screenshots || [];
    body.innerHTML = `
      <div class="shots-grid">
        ${shots.map((s, i) => `
          <button class="shot-thumb" data-shot="${i}">
            <img src="${escapeAttr(s)}" alt="screenshot ${i+1}" loading="lazy">
          </button>`).join('')}
      </div>
    `;
    body.querySelectorAll('.shot-thumb').forEach(b => b.addEventListener('click', () => openLightbox(+b.dataset.shot)));
  }
  else if (currentTab === 'history') {
    const rels = a.releases || [];
    body.innerHTML = `
      <div class="history-list">
        ${rels.map(r => {
          const n = getNotes(r);
          const isLatest = r.version === a.available;
          return `
          <article class="history-item">
            <header class="history-head">
              <span class="history-ver">v${escapeHtml(r.version)}</span>
              ${isLatest ? `<span class="history-badge">${i18n('detail.latestBadge')}</span>` : ''}
              ${r.released ? `<span class="history-date">${escapeHtml(r.released)}</span>` : ''}
              ${r.channel && r.channel !== 'stable' ? `<span class="history-channel">${escapeHtml(r.channel)}</span>` : ''}
            </header>
            ${n ? `<div class="history-notes">${formatMd(n)}</div>` : `<div class="history-notes muted">${i18n('detail.noNotes')}</div>`}
          </article>`;
        }).join('')}
      </div>
    `;
  }
}

// Tiny markdown: bold **x**, links, line breaks. Safer than dropping innerHTML raw.
function formatMd(s) {
  if (!s) return '';
  // Escape first, then re-apply safe tags.
  let out = escapeHtml(s);
  out = out.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/`([^`]+)`/g, '<code>$1</code>');
  // Bullet lines starting with "- "
  out = out.replace(/^- (.+)$/gm, '• $1');
  // Paragraphs from double newline, <br> from single
  out = out.split(/\n{2,}/).map(p => '<p>' + p.replace(/\n/g, '<br>') + '</p>').join('');
  return out;
}

function escapeAttr(s){ return escapeHtml(s); }

function svgIcon(name) {
  const stroke = 'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"';
  switch (name) {
    case 'download': return `<svg width="14" height="14" viewBox="0 0 24 24" ${stroke}><path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/></svg>`;
    case 'refresh':  return `<svg width="14" height="14" viewBox="0 0 24 24" ${stroke}><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>`;
    case 'play':     return `<svg width="14" height="14" viewBox="0 0 24 24" ${stroke}><polygon points="6 4 20 12 6 20 6 4" fill="currentColor" stroke="none"/></svg>`;
    case 'trash':    return `<svg width="14" height="14" viewBox="0 0 24 24" ${stroke}><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`;
    case 'check':    return `<svg width="14" height="14" viewBox="0 0 24 24" ${stroke}><polyline points="20 6 9 17 4 12"/></svg>`;
    case 'back':     return `<svg width="16" height="16" viewBox="0 0 24 24" ${stroke}><path d="M19 12H5"/><path d="m12 19-7-7 7-7"/></svg>`;
    default: return '';
  }
}

// ───── LIGHTBOX ─────
function openLightbox(idx) {
  if (!currentApp || !currentApp.screenshots) return;
  lightboxIdx = idx;
  let lb = $('#lightbox');
  if (!lb) {
    lb = document.createElement('div');
    lb.id = 'lightbox';
    lb.className = 'lightbox hidden';
    lb.innerHTML = `
      <button class="lb-close" id="lb-close" aria-label="Close">×</button>
      <button class="lb-prev" id="lb-prev" aria-label="Prev">‹</button>
      <img id="lb-img" alt="">
      <button class="lb-next" id="lb-next" aria-label="Next">›</button>
      <div class="lb-counter" id="lb-counter"></div>
    `;
    document.body.appendChild(lb);
    lb.querySelector('#lb-close').addEventListener('click', closeLightbox);
    lb.querySelector('#lb-prev').addEventListener('click', () => stepLightbox(-1));
    lb.querySelector('#lb-next').addEventListener('click', () => stepLightbox(+1));
    lb.addEventListener('click', e => { if (e.target === lb) closeLightbox(); });
  }
  updateLightbox();
  lb.classList.remove('hidden');
}
function updateLightbox() {
  const lb = $('#lightbox'); if (!lb) return;
  const shots = currentApp.screenshots || [];
  lb.querySelector('#lb-img').src = shots[lightboxIdx] || '';
  lb.querySelector('#lb-counter').textContent = `${lightboxIdx+1} / ${shots.length}`;
  lb.querySelector('#lb-prev').style.visibility = lightboxIdx > 0 ? 'visible' : 'hidden';
  lb.querySelector('#lb-next').style.visibility = lightboxIdx < shots.length-1 ? 'visible' : 'hidden';
}
function stepLightbox(d) {
  const shots = currentApp.screenshots || [];
  lightboxIdx = Math.max(0, Math.min(shots.length-1, lightboxIdx + d));
  updateLightbox();
}
function closeLightbox() { lightboxIdx = -1; $('#lightbox')?.classList.add('hidden'); }

// ───── ACTIONS ─────
async function doLaunch(app) {
  try { await invoke('launch_app', { pkg: app.pkg }); toast(i18n('launching', { app: getLabel(app) })); }
  catch (e) { toast(i18n('err', { m: e })); }
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
      const fill = $('#detail-progress-fill');
      if (fill) { fill.style.width = pct.toFixed(1)+'%'; fill.style.animation = 'none'; }
      const txt = $('#detail-progress-text');
      if (txt && p.total > 0) {
        txt.textContent = i18n('downloadingPct', { pct: pct.toFixed(0), dn: fmtBytes(p.downloaded), total: fmtBytes(p.total) });
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
  const pmName = pmInfo.pm || 'pacman';
  const asset = (app.release_assets || []).find(a => assetCompat(a, pmName));
  if (!asset) {
    toast(i18n('noPkg', { exts: (pmInfo.exts||['.pkg.tar.zst']).join('/') }));
    return;
  }
  const pwd = await promptAuth(
    isUpdate ? i18n('auth.titleUpdate') : i18n('auth.titleInstall'),
    i18n(isUpdate ? 'auth.descUpdate' : 'auth.descInstall', { app: getLabel(app) }));
  if (pwd === null) return;
  const opId = newOpId();
  currentOpId = opId;
  setProgress(true, i18n('downloadingFile', { f: asset.name }), true);
  startProgressPoll(opId);
  let stage = i18n('stage.download');
  try {
    const localPath = await invoke('download_pkg', { url: asset.browser_download_url, filename: asset.name, opId });
    stopProgressPoll();
    setProgress(true, i18n('installingWith', { pm: pmInfo.pm || 'pm' }), false);
    stage = i18n('stage.install');
    await invoke('install_pkg_file', { path: localPath, password: pwd, opId });
    currentOpId = null;
    if (app.self && isUpdate) {
      toast(i18n('selfUpdate'));
      setTimeout(() => tauriWin().close(), 1400);
      return;
    }
    toast(i18n(isUpdate?'updatedToast':'installedToast', { app: getLabel(app) }));
    await loadApps();
    if (currentApp) {
      const refreshed = allApps.find(x => x.pkg === currentApp.pkg);
      if (refreshed) { Object.assign(currentApp, refreshed); render(); }
    }
  } catch (e) {
    stopProgressPoll();
    currentOpId = null;
    const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
    if (msg.includes('__cancelled__')) toast(i18n('cancelledOp'));
    else showErrorDialog(stage, asset.name, msg);
    setProgress(false);
  }
}

function showErrorDialog(stage, file, msg) {
  const dlg = document.createElement('div');
  dlg.style.cssText = 'position:fixed;inset:0;background:rgba(0,0,0,.7);display:flex;align-items:center;justify-content:center;z-index:1000;padding:20px';
  dlg.innerHTML = `<div style="background:var(--card);border:1px solid var(--brd);border-radius:14px;padding:24px;max-width:600px;width:100%;max-height:80vh;overflow:auto">
    <h3 style="font-size:16px;font-weight:700;margin-bottom:8px;color:var(--red)">${i18n('errIn', { stage })}</h3>
    <p style="font-size:13px;color:var(--tx2);margin-bottom:12px">${escapeHtml(file)}</p>
    <pre style="background:var(--sbg);padding:12px;border-radius:8px;font-family:monospace;font-size:11.5px;white-space:pre-wrap;word-break:break-word;color:var(--tx);max-height:300px;overflow:auto">${escapeHtml(msg)}</pre>
    <button id="dlg-close" style="margin-top:14px;background:var(--blue);color:#fff;border:0;padding:8px 18px;border-radius:9px;cursor:pointer;font-family:inherit;font-weight:600">${i18n('close')}</button>
  </div>`;
  document.body.appendChild(dlg);
  dlg.querySelector('#dlg-close').addEventListener('click', () => dlg.remove());
  dlg.addEventListener('click', e => { if (e.target === dlg) dlg.remove(); });
}

async function doUninstall(app) {
  if (!confirm(i18n('uninstallConfirm', { app: getLabel(app) }))) return;
  const pwd = await promptAuth(i18n('auth.titleUninstall'),
    i18n('auth.descUninstall', { app: getLabel(app), pkg: app.pkg }));
  if (pwd === null) return;
  const opId = newOpId();
  currentOpId = opId;
  setProgress(true, i18n('uninstalling'), true);
  try {
    await invoke('uninstall_pkg', { pkg: app.pkg, password: pwd, opId });
    currentOpId = null;
    toast(i18n('uninstalledToast', { app: getLabel(app) }));
    await loadApps();
    if (currentApp) {
      const refreshed = allApps.find(x => x.pkg === currentApp.pkg);
      if (refreshed) { Object.assign(currentApp, refreshed); render(); }
    }
  } catch (e) {
    currentOpId = null;
    const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
    if (msg.includes('__cancelled__')) toast(i18n('cancelledOp'));
    else toast(i18n('err', { m: msg.slice(0,140) }));
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
  const pwd = await promptAuth(i18n('auth.titleUpdateAll'),
    i18n('auth.descUpdateAll', { n: list.length }));
  if (pwd === null) return;
  let needsRestart = false;
  for (const app of list) {
    const pmName = pmInfo.pm || 'pacman';
    const asset = (app.release_assets || []).find(a => assetCompat(a, pmName));
    if (!asset) { toast(i18n('skipNoPkg', { app: getLabel(app) })); continue; }
    const opId = newOpId();
    currentOpId = opId;
    setProgress(true, i18n('downloadingFile', { f: asset.name }), true);
    startProgressPoll(opId);
    try {
      const localPath = await invoke('download_pkg', { url: asset.browser_download_url, filename: asset.name, opId });
      stopProgressPoll();
      setProgress(true, i18n('installingApp', { app: getLabel(app) }), false);
      await invoke('install_pkg_file', { path: localPath, password: pwd, opId });
      toast(i18n('updatedToast', { app: getLabel(app) }));
      if (app.self) needsRestart = true;
    } catch (e) {
      stopProgressPoll();
      const msg = typeof e === 'string' ? e : (e.message || JSON.stringify(e));
      if (msg.includes('__cancelled__')) { toast(i18n('cancelled')); break; }
      toast(i18n('err', { m: getLabel(app) + ': ' + msg.slice(0,100) }));
    }
  }
  currentOpId = null;
  setProgress(false);
  if (needsRestart) {
    toast(i18n('selfUpdate'));
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
      if (!p) { err.textContent = i18n('auth.emptyPass'); err.classList.remove('hidden'); return; }
      okBtn.disabled = true;
      const ok = await invoke('verify_password', { password: p });
      okBtn.disabled = false;
      if (!ok) { err.textContent = i18n('auth.err'); err.classList.remove('hidden'); input.select(); return; }
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
  const p = $('#detail-progress');
  if (!p) return;
  p.classList.toggle('hidden', !on);
  if (text) $('#detail-progress-text').textContent = text;
  const fill = $('#detail-progress-fill');
  if (fill) {
    if (cancellable) { fill.style.animation = 'none'; fill.style.width = '0%'; }
    else { fill.style.animation = ''; fill.style.width = ''; }
  }
  document.querySelectorAll('.detail-action').forEach(b => b.disabled = on);
  const cBtn = $('#detail-cancel');
  if (cBtn) cBtn.classList.toggle('hidden', !(on && cancellable));
}

// ───── WIRE ─────
function wire() {
  $('#minimize').addEventListener('click', () => tauriWin().minimize());
  $('#maximize').addEventListener('click', () => tauriWin().toggleMaximize());
  $('#close').addEventListener('click', () => tauriWin().close());
  $('#theme-btn').addEventListener('click', cycleTheme);
  const langBtn = $('#lang-btn');
  if (langBtn) langBtn.addEventListener('click', cycleLang);
  $('#refresh-btn').addEventListener('click', async () => {
    try { await invoke('clear_catalog_cache'); } catch {}
    iconCache.clear();
    toast(i18n('refreshing'));
    await loadApps();
  });
  $('#update-all-btn').addEventListener('click', () => doUpdateAll());
  $('#search').addEventListener('input', () => { if (!currentApp) renderGrid(); });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if (lightboxIdx >= 0) { closeLightbox(); return; }
      if (currentApp) closeDetail();
    }
    if ((e.ctrlKey||e.metaKey) && e.key === 'f') { e.preventDefault(); $('#search').focus(); }
    if (lightboxIdx >= 0 && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      stepLightbox(e.key === 'ArrowLeft' ? -1 : +1);
    }
  });
  document.addEventListener('click', e => {
    const c = e.target.closest?.('#detail-cancel');
    if (c) cancelCurrentOp();
  });
}

(async function init(){
  if (window.BookosI18n) BookosI18n.setLang(uiLang);
  await applyTheme();
  wire();
  await loadApps();
})();
