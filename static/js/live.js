// Every 30 s while the tab is visible, fetches the page again and swaps in the elements marked
// `data-live` (each needs an id) that changed. Keeps the focused link or summary and the <details>
// left open or closed (matched by id), and reads out a changed `data-announce` text (the status
// sentence) in a polite live region. A redirect means the session expired, a 404 that the monitor
// was deleted: reload to show it.
const PERIOD_MS = 30_000;

const announcer = document.createElement("p");
announcer.className = "visually-hidden";
announcer.setAttribute("aria-live", "polite");
document.body.append(announcer);

// The bars rise once on load (CSS); blocks swapped in later stay still.
setTimeout(() => document.documentElement.classList.add("settled"), 1000);

const announced = (root) => root.querySelector("[data-announce]")?.textContent.trim() ?? "";

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
    const page = new DOMParser().parseFromString(await response.text(), "text/html");
    const active = document.activeElement;
    const focusedHref = active?.getAttribute("href");
    const focusedDetails = active?.matches("summary") ? active.parentElement.id : "";
    for (const element of document.querySelectorAll("[data-live]")) {
        const next = page.getElementById(element.id);
        if (!next) {
            continue;
        }
        for (const details of element.querySelectorAll("details[id]")) {
            const twin = next.querySelector(`#${CSS.escape(details.id)}`);
            if (twin) {
                twin.open = details.open;
            }
        }
        if (next.outerHTML === element.outerHTML) {
            continue;
        }
        const refocus = element.contains(active);
        const before = announced(element);
        element.replaceWith(next);
        const after = announced(next);
        if (after && after !== before) {
            announcer.textContent = after;
        }
        if (refocus && focusedDetails) {
            next.querySelector(`#${CSS.escape(focusedDetails)} > summary`)?.focus();
        } else if (refocus && focusedHref) {
            next.querySelector(`[href="${CSS.escape(focusedHref)}"]`)?.focus();
        }
    }
    document.dispatchEvent(new Event("live:refreshed"));
}

setInterval(refresh, PERIOD_MS);
document.addEventListener("visibilitychange", refresh);
