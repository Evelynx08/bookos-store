// Cliente del color dinámico de BookOS — compartido por todas las apps.
//
// Ajustes genera la paleta desde el fondo de pantalla y la deja en
// ~/.config/bookos/palette.css. Este script la mete en un <style> que va
// SIEMPRE al final de <head>: mismas variables, misma especificidad y más
// tarde en la cascada, así que gana sin tocar una regla de la app.
//
// Se carga como script normal (no módulo) desde el <head>, DESPUÉS del
// <link rel="stylesheet">:
//     <script src="bookos-palette.js"></script>
//
// Requiere en el backend de la app un único comando:
//     #[tauri::command] fn bookos_palette_css() -> String
//
// Fichero idéntico en todas las apps. Si se cambia aquí, cópiese tal cual;
// el original vive en BookOS-Settings/src-tauri/extra/dynamic-color/.

(function () {
    'use strict';
    var STYLE_ID = 'bookos-dynamic-palette';
    var CACHE_KEY = '__bookos_palette_css__';

    function install(css) {
        var el = document.getElementById(STYLE_ID);
        if (!css) {
            if (el) el.remove();
            try { localStorage.removeItem(CACHE_KEY); } catch (e) {}
            return;
        }
        if (!el) { el = document.createElement('style'); el.id = STYLE_ID; }
        el.textContent = css;
        // Reinsertar al final aunque ya existiera: cualquier hoja añadida
        // después quedaría por encima y ganaría el desempate de la cascada.
        (document.head || document.documentElement).appendChild(el);
        try { localStorage.setItem(CACHE_KEY, css); } catch (e) {}
    }

    // Primero la copia en caché, de forma síncrona: esperar al backend haría
    // que la ventana abriese con los colores de fábrica y saltase al color del
    // fondo medio segundo después, que se ve peor que no teñir nada.
    try {
        var cached = localStorage.getItem(CACHE_KEY);
        if (cached) install(cached);
    } catch (e) {}

    function sync() {
        var inv = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
        if (!inv) return;
        inv('bookos_palette_css')
            .then(function (css) { install(css || ''); })
            .catch(function () {});
    }

    // Al acabar de analizar el HTML se vuelve a colocar la hoja al final. El
    // script corre en mitad del <head>, así que cualquier <link> que venga
    // DESPUÉS quedaría por detrás en el DOM y ganaría el desempate; reinsertar
    // aquí la devuelve al último lugar, ya con todas las hojas cargadas.
    function reseat() {
        var el = document.getElementById(STYLE_ID);
        if (el) document.head.appendChild(el);
        sync();
    }
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', reseat);
    } else {
        reseat();
    }
    // La paleta puede cambiar mientras la app está abierta (el usuario toca
    // otro color en Ajustes, o cambia el fondo). Volver a leerla al recuperar
    // el foco cuesta una IPC y evita tener que montar un vigilante de ficheros
    // en siete aplicaciones.
    window.addEventListener('focus', sync);
})();
