// Every 30 s while the tab is visible, fetches the page again and swaps in the elements marked
// `data-live` (each needs an id) that changed. Keeps the focused link or summary and the <details>
// left open or closed (matched by id), and reads out a changed `data-announce` text (the status
// sentence) in a polite live region. A redirect means the session expired, a 404 that the monitor
// was deleted: reload to show it.
// A form being typed in (a text field focused, or changed and not saved yet) is carried over into
// the swapped block, matched by id: the rest of the block still refreshes and the typing is kept.
// Boxes ticked or unticked elsewhere are set again on their twins, matched by name and value, so a
// selection does not stop the rows from refreshing.
const PERIOD_MS = 30_000;

const announcer = document.createElement("p");
announcer.className = "visually-hidden";
announcer.setAttribute("aria-live", "polite");
document.body.append(announcer);

// The bars rise once on load (CSS); blocks swapped in later stay still.
setTimeout(() => document.documentElement.classList.add("settled"), 1000);

const editing = (form) =>
    [...form.querySelectorAll("textarea, input:not([type=hidden], [type=checkbox], [type=radio])")].some(
        (field) => field === document.activeElement || field.value !== field.defaultValue,
    );

// The boxes changed since load, outside the kept forms.
const ticks = (root, kept) =>
    [...root.querySelectorAll("input[type=checkbox][name], input[type=radio][name]")].filter(
        (box) => box.checked !== box.defaultChecked && !kept.some((form) => form.contains(box)),
    );

const twinBox = (root, box) =>
    root.querySelector(`input[name="${CSS.escape(box.name)}"][value="${CSS.escape(box.value)}"]`);

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
        const kept = [...element.querySelectorAll("form[id]")].filter(editing);
        if (next.outerHTML === element.outerHTML) {
            continue;
        }
        const changed = ticks(element, kept);
        const field = kept.some((form) => form.contains(active)) ? active : null;
        const selection = field ? [field.selectionStart, field.selectionEnd] : null;
        for (const form of kept) {
            // Not `form.id`: a field named "id" (the dashboard's row boxes) shadows it.
            next.querySelector(`#${CSS.escape(form.getAttribute("id"))}`)?.replaceWith(form);
        }
        for (const box of changed) {
            const match = twinBox(next, box);
            if (match) {
                match.checked = box.checked;
            }
        }
        const refocus = element.contains(active);
        const before = announced(element);
        element.replaceWith(next);
        const after = announced(next);
        if (after && after !== before) {
            announcer.textContent = after;
        }
        if (field && next.contains(field)) {
            field.focus();
            if (selection[0] !== null) {
                field.setSelectionRange(...selection);
            }
        } else if (refocus && changed.includes(active)) {
            twinBox(next, active)?.focus();
        } else if (refocus && focusedDetails) {
            next.querySelector(`#${CSS.escape(focusedDetails)} > summary`)?.focus();
        } else if (refocus && focusedHref) {
            next.querySelector(`[href="${CSS.escape(focusedHref)}"]`)?.focus();
        }
    }
    document.dispatchEvent(new Event("live:refreshed"));
}

setInterval(refresh, PERIOD_MS);
document.addEventListener("visibilitychange", refresh);
