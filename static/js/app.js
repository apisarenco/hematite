// Keep track of the currently rendered pathname & search
let currentRenderedPath = location.pathname + location.search;

// SPA Client Navigation Handler
document.addEventListener('click', async (e) => {
    const link = e.target.closest('a');
    if (!link) return;
    const href = link.getAttribute('href');
    if (!href || href.startsWith('http:') || href.startsWith('https:') || href.startsWith('//') || href.startsWith('mailto:') || href.startsWith('tel:') || href.startsWith('/_user') || href.startsWith('/_admin') || href.startsWith('/_static') || href.startsWith('/_search')) return;

    // Use location.href as the base to resolve relative links and anchors correctly against current page
    const targetUrl = new URL(href, location.href);

    // If only the hash/anchor changes on the current page, scroll to element without reloading content
    if (targetUrl.pathname === location.pathname && targetUrl.search === location.search) {
        if (targetUrl.hash) {
            const hashTargetId = decodeURIComponent(targetUrl.hash.slice(1));
            const targetElement = document.getElementById(hashTargetId);
            if (targetElement) {
                e.preventDefault();
                targetElement.scrollIntoView({ behavior: 'smooth' });
                history.pushState(null, '', href);
            }
        }
        return;
    }

    e.preventDefault();
    e.stopPropagation();
    const newPath = targetUrl.pathname + targetUrl.search;
    history.pushState(null, '', href);
    await loadSpaPage(targetUrl.pathname, targetUrl.hash);
});

window.addEventListener('popstate', async (e) => {
    const targetUrl = new URL(location.href);
    const targetPath = targetUrl.pathname + targetUrl.search;

    if (targetPath === currentRenderedPath) {
        if (targetUrl.hash) {
            const hashTargetId = decodeURIComponent(targetUrl.hash.slice(1));
            const targetElement = document.getElementById(hashTargetId);
            if (targetElement) {
                targetElement.scrollIntoView({ behavior: 'smooth' });
            }
        }
        return;
    }

    await loadSpaPage(targetUrl.pathname, targetUrl.hash);
});

// Update active link and expand all ancestor directories up to the active page
function updateActiveSidebar(currentPath) {
    const navEl = document.querySelector('.nav-tree');
    if (!navEl) return;

    // Remove existing active states
    navEl.querySelectorAll('a.active').forEach(a => a.classList.remove('active'));

    const normTarget = currentPath.replace(/\/$/, '');
    const links = navEl.querySelectorAll('a[href]');
    let targetLink = null;
    for (const a of links) {
        const href = (a.getAttribute('href') || '').replace(/\/$/, '');
        if (href === normTarget) {
            targetLink = a;
            break;
        }
    }

    if (targetLink) {
        targetLink.classList.add('active');
        // Expand all ancestor <details class="nav-folder"> up to the nav tree root
        let parent = targetLink.parentElement;
        while (parent && parent !== navEl) {
            if (parent.tagName === 'DETAILS' && parent.classList.contains('nav-folder')) {
                parent.open = true;
            }
            parent = parent.parentElement;
        }
        targetLink.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
}

// Run on initial load
updateActiveSidebar(location.pathname);

async function loadSpaPage(path, hash) {
    try {
        const res = await fetch(`/_api/v1/page?path=${encodeURIComponent(path)}`);
        const data = await res.json();

        if (!res.ok && !data.regions) {
            window.location.href = path + (hash || ''); // Fallback to normal navigation
            return;
        }

        document.title = (data.title ? data.title + " - Hematite" : "Hematite");
        if (data.regions) {
            if (data.regions.content) {
                const contentEl = document.getElementById('region-content');
                if (contentEl) contentEl.innerHTML = data.regions.content;
            }
            if (data.regions.toc !== undefined) {
                const tocEl = document.getElementById('region-toc');
                if (tocEl) tocEl.innerHTML = data.regions.toc;
            }
            if (data.regions.breadcrumbs) {
                const breadcrumbsEl = document.getElementById('region-breadcrumbs');
                if (breadcrumbsEl) breadcrumbsEl.innerHTML = data.regions.breadcrumbs;
            }
        }
        // Update active link in sidebar and expand directory ancestors without resetting user folder state
        updateActiveSidebar(path);
        currentRenderedPath = path;

        if (hash) {
            const targetId = decodeURIComponent(hash.slice(1));
            const el = document.getElementById(targetId);
            if (el) {
                el.scrollIntoView({ behavior: 'smooth' });
                return;
            }
        }
        window.scrollTo(0, 0);
    } catch (err) {
        console.error("SPA swap failed, falling back to full load", err);
        window.location.href = path + (hash || '');
    }
}

// Live FTS5 Search
const searchInput = document.getElementById('search-input');
const searchResults = document.getElementById('search-results');
if (searchInput) {
    let timeout = null;
    searchInput.addEventListener('input', (e) => {
        clearTimeout(timeout);
        const q = e.target.value.trim();
        if (!q) {
            searchResults.style.display = 'none';
            searchResults.innerHTML = '';
            return;
        }
        timeout = setTimeout(async () => {
            const res = await fetch(`/_api/v1/search?q=${encodeURIComponent(q)}`);
            if (res.ok) {
                const hits = await res.json();
                if (hits.length > 0) {
                    searchResults.style.display = 'block';
                    searchResults.innerHTML = hits.map(h => `<div style="padding:0.25rem 0;"><a href="${h.url}">${h.title}</a></div>`).join('');
                } else {
                    searchResults.style.display = 'block';
                    searchResults.innerHTML = '<span style="font-size:0.8rem; color:#868e96;">No results</span>';
                }
            }
        }, 200);
    });
}
