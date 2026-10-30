// Bulk actions on the dashboard. Until a row is ticked the bar only offers "Select all", which
// ticks the rows on screen; then it shows the count and the actions. A row the filter hides or a
// folded group covers is unticked, so an action only touches what is on screen. Without JS the
// whole bar is always there. The form lives in the refreshed block: listen above it.
const shown = (box) => !box.closest("li[hidden], details[hidden], details:not([open])");
const rowBoxes = () => [...document.querySelectorAll("#bulk-form .row-select")];

function update() {
    const form = document.getElementById("bulk-form");
    if (!form) {
        return;
    }
    const boxes = rowBoxes();
    for (const box of boxes) {
        if (!shown(box)) {
            box.checked = false;
        }
    }
    const visible = boxes.filter(shown);
    const ticked = visible.filter((box) => box.checked).length;
    const all = form.querySelector("[data-select-all]");
    const toggle = all.querySelector("input");
    all.hidden = false;
    toggle.checked = ticked > 0 && ticked === visible.length;
    toggle.indeterminate = ticked > 0 && ticked < visible.length;
    all.querySelector("[data-selected-count]").textContent = ticked ? `${ticked} selected` : "Select all";
    form.querySelector("[data-bulk-bar]").toggleAttribute("data-empty", ticked === 0);
}

document.addEventListener("change", (event) => {
    if (event.target.closest("#bulk-form [data-select-all]")) {
        for (const box of rowBoxes().filter(shown)) {
            box.checked = event.target.checked;
        }
    }
    if (event.target.closest("#bulk-form")) {
        update();
    }
});
// `toggle` does not bubble: catch it on the way down.
document.addEventListener("toggle", (event) => event.target.matches?.("details[data-group]") && update(), true);

// The notice of the last action is read once: a reload or a bookmark must not show it again.
const params = new URLSearchParams(location.search);
if (params.has("done")) {
    params.delete("done");
    params.delete("count");
    history.replaceState(history.state, "", params.size ? `/?${params}` : "/");
}

update();
document.addEventListener("live:refreshed", update);
document.addEventListener("dashboard:filtered", update);
