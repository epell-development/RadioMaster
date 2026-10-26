// The header menu on phones: the ☰ button opens and closes the links. Without this script the links
// simply wrap onto a second row, so nothing is out of reach.
(function () {
  var head = document.querySelector('header.top');
  var btn = head && head.querySelector('.menu-btn');
  var nav = document.getElementById('site-nav');
  if (!btn || !nav) return;
  head.classList.add('has-menu');

  function set(open) {
    head.classList.toggle('open', open);
    btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    btn.setAttribute('aria-label', open ? 'Close menu' : 'Open menu');
  }
  btn.addEventListener('click', function () { set(!head.classList.contains('open')); });
  nav.addEventListener('click', function (e) { if (e.target.closest('a')) set(false); });
  document.addEventListener('click', function (e) { if (!head.contains(e.target)) set(false); });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && head.classList.contains('open')) { set(false); btn.focus(); }
  });
  var wide = window.matchMedia('(min-width: 761px)');
  var close = function (m) { if (m.matches) set(false); };
  if (wide.addEventListener) wide.addEventListener('change', close); else if (wide.addListener) wide.addListener(close);
})();
