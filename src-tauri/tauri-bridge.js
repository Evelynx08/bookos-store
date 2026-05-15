// Tauri 2 bridge for external URLs (http://localhost:PORT).
(function () {
  if (typeof window === 'undefined') return;
  if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.window) return;
  const internals = window.__TAURI_INTERNALS__;
  if (!internals) { console.warn('[tauri-bridge] __TAURI_INTERNALS__ missing'); return; }
  const invoke = (cmd, args) => internals.invoke(cmd, args || {});
  function makeWindowProxy(label) {
    const w = {
      label,
      _call(c, p) { return invoke('plugin:window|' + c, Object.assign({ label }, p || {})); },
      minimize() { return this._call('minimize'); },
      maximize() { return this._call('maximize'); },
      unmaximize() { return this._call('unmaximize'); },
      toggleMaximize() { return this._call('toggle_maximize'); },
      close() { return this._call('close'); },
      show() { return this._call('show'); },
      hide() { return this._call('hide'); },
      setFocus() { return this._call('set_focus'); },
      setTitle(title) { return this._call('set_title', { title }); },
      setFullscreen(v) { return this._call('set_fullscreen', { value: !!v }); },
      isMaximized() { return this._call('is_maximized'); },
      startDragging() { return this._call('start_dragging'); },
      outerPosition() { return this._call('outer_position'); },
    };
    return w;
  }
  const cur = makeWindowProxy('main');
  window.__TAURI__ = {
    core: { invoke, convertFileSrc: (p) => internals.convertFileSrc ? internals.convertFileSrc(p) : p },
    window: { getCurrentWindow: () => cur, Window: { getCurrent: () => cur } },
    dialog: {
      open: (o) => invoke('plugin:dialog|open', o || {}),
      message: (m, o) => invoke('plugin:dialog|message', Object.assign({ message: m }, o || {})),
      ask: (m, o) => invoke('plugin:dialog|ask', Object.assign({ message: m }, o || {})),
      confirm: (m, o) => invoke('plugin:dialog|confirm', Object.assign({ message: m }, o || {})),
    },
  };
})();
