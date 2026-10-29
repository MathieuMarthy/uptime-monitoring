// Every 30 s while the tab is visible, fetches the page again and swaps in the elements marked
// `data-live` (each needs an id). A redirect means the session expired: reload to log in again.
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
    if (response.redirected) {
        location.reload();
        return;
    }
    if (!response.ok) {
        return;
    }
    const fresh = new DOMParser().parseFromString(await response.text(), "text/html");
    for (const element of document.querySelectorAll("[data-live]")) {
        const next = fresh.getElementById(element.id);
        if (next) {
            element.replaceWith(next);
        }
    }
    document.dispatchEvent(new Event("live:refreshed"));
}

setInterval(refresh, PERIOD_MS);
document.addEventListener("visibilitychange", refresh);
