// Every 30 s while the tab is visible, fetches the page again and swaps in the elements marked
// `data-live` (each needs an id), keeping the focused link. A redirect means the session expired,
// a 404 that the monitor was deleted: reload to show it.
const PERIOD_MS = 30_000;

async function refresh() {
    if (document.hidden) {
        return;
    }
    let response;
    try {
        response = await fetch(location.href, { cache: "no-store" });
    } catch {
        return; // offline: the next tick tries again
    }
    if (response.redirected || response.status === 404) {
        location.reload();
        return;
    }
    if (!response.ok) {
        return;
    }
    const fresh = new DOMParser().parseFromString(await response.text(), "text/html");
    const focused = document.activeElement?.getAttribute("href");
    for (const element of document.querySelectorAll("[data-live]")) {
        const next = fresh.getElementById(element.id);
        if (!next) {
            continue;
        }
        const refocus = focused && element.contains(document.activeElement);
        element.replaceWith(next);
        if (refocus) {
            next.querySelector(`[href="${CSS.escape(focused)}"]`)?.focus();
        }
    }
    document.dispatchEvent(new Event("live:refreshed"));
}

setInterval(refresh, PERIOD_MS);
document.addEventListener("visibilitychange", refresh);
