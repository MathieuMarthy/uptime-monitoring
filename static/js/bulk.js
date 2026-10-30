// Bulk actions on the dashboard. Until a row is ticked the bar only offers "Select all", which
// ticks the rows the filter shows; then it shows the count and the actions. Rows hidden by the
// filter are left out when the form is sent, so an action only touches what is on screen.
// Without JS the whole bar is always there. The form lives in the refreshed block: listen above it.
const shown = (box) => !box.closest("li[hidden], details[hidden]");
const rowBoxes = () => [...document.querySelectorAll("#bulk-form .row-select")];

function update() {
    const form = document.getElementById("bulk-form");
    if (!form) {
        return;
    }
    const boxes = rowBoxes().filter(shown);
    const ticked = boxes.filter((box) => box.checked).length;
    const all = form.querySelector("[data-select-all]");
    const toggle = all.querySelector("input");
    all.hidden = false;
    toggle.checked = ticked > 0 && ticked === boxes.length;
    toggle.indeterminate = ticked > 0 && ticked < boxes.length;
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

document.addEventListener("submit", (event) => {
    if (event.target.id !== "bulk-form") {
        return;
    }
    const left = rowBoxes().filter((box) => !shown(box) && !box.disabled);
    left.forEach((box) => (box.disabled = true));
    // Back once the form data is taken, or after a cancelled confirmation.
    setTimeout(() => left.forEach((box) => (box.disabled = false)));
});

update();
document.addEventListener("live:refreshed", update);
document.addEventListener("dashboard:filtered", update);
