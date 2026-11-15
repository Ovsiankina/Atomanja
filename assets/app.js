/* Atomanja client script: registers the service worker and wires the
   "Enable notifications" button of the settings page (APP-33). Pages work
   without it; htmx is optional too. */
(function () {
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/sw.js');
  }

  // Task views (TaskViews README): Gantt on desktop, list on a phone
  // until the person picks one (the server remembers it in a cookie).
  var views = document.querySelector('.at-views[data-layout]');
  if (views && location.pathname === '/' && !/[?&]view=/.test(location.search) &&
      document.cookie.indexOf('atomanja_view=') < 0 &&
      window.matchMedia && window.matchMedia('(max-width: 520px)').matches &&
      views.getAttribute('data-layout') === 'gantt') {
    location.replace('/?view=list');
    return;
  }
  // The Gantt opens scrolled so today is in view.
  var gantt = document.querySelector('.at-gantt');
  var today = gantt && gantt.querySelector('.at-gantt__day.is-today');
  if (today) {
    var label = gantt.querySelector('.at-gantt__label');
    gantt.scrollLeft = Math.max(0, today.offsetLeft - (label ? label.offsetWidth : 0) - 7 * today.offsetWidth);
  }

  var btn = document.getElementById('enable-push');
  if (!btn) return;
  var hint = document.getElementById('push-hint');
  var key = btn.getAttribute('data-key');

  function bytes(b64) {
    // Pad to a multiple of 4: length%4 of 2 needs "==", 3 needs "=", 0 none.
    // The old formula over-padded the 87-char VAPID key and atob threw.
    var s = (b64 + '===='.slice(b64.length % 4 || 4)).replace(/-/g, '+').replace(/_/g, '/');
    var raw = atob(s), out = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out;
  }

  function done() {
    btn.disabled = true;
    btn.firstChild.textContent = btn.getAttribute('data-done');
    if (hint) hint.hidden = true;
  }

  // The button stays disabled until push can work: iPhone only allows it
  // from the Home Screen app; other browsers allow it in a tab.
  var standalone = navigator.standalone === true ||
    (window.matchMedia && window.matchMedia('(display-mode: standalone)').matches);
  var ios = /iPhone|iPad|iPod/.test(navigator.userAgent);
  if (!key || !('serviceWorker' in navigator) || !('PushManager' in window) || (ios && !standalone)) return;

  navigator.serviceWorker.ready.then(function (reg) {
    return reg.pushManager.getSubscription();
  }).then(function (sub) {
    if (sub && Notification.permission === 'granted') { done(); return; }
    btn.disabled = false;
    if (hint) hint.hidden = true;
  });

  // Every failure is shown under the button: a silent one looks like a
  // dead button on a phone, where there is no console.
  function fail(what) {
    btn.disabled = false;
    if (hint) { hint.hidden = false; hint.textContent = '\u26a0 ' + what; }
  }

  btn.addEventListener('click', function () {
    btn.disabled = true;
    if (hint) { hint.hidden = false; hint.textContent = '\u2026'; }
    Notification.requestPermission().then(function (perm) {
      if (perm !== 'granted') { fail('permission: ' + perm); return null; }
      return navigator.serviceWorker.ready.then(function (reg) {
        return reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: bytes(key) });
      }).then(function (sub) {
        return fetch('/push/subscribe', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify(sub)
        });
      }).then(function (res) {
        if (res.ok) done(); else fail('server: ' + res.status);
      });
    }).catch(function (e) { fail((e && e.message) || String(e)); });
  });
})();
