// Shared site navigation for the static pages that do not load script.js.
// Without it the link row is display:none below 900px (see the mobile nav
// block in style.css) and a phone reader has no navigation at all.
(function () {
    const toggle = document.getElementById('nav-toggle');
    const links = document.getElementById('nav-links');
    if (!toggle || !links) return;

    const setNav = (open) => {
        links.classList.toggle('open', open);
        toggle.setAttribute('aria-expanded', String(open));
        toggle.setAttribute('aria-label', open ? 'Close menu' : 'Open menu');
        toggle.innerHTML = `<i data-lucide="${open ? 'x' : 'menu'}"></i>`;
        if (typeof lucide !== 'undefined' && lucide.createIcons) lucide.createIcons();
    };

    toggle.addEventListener('click', () => setNav(!links.classList.contains('open')));
    links.addEventListener('click', (e) => {
        if (e.target.closest('a')) setNav(false);
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && links.classList.contains('open')) setNav(false);
    });
})();
