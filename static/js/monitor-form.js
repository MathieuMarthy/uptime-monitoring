// Shows the fields of the chosen monitor type. Hidden fields are disabled: the browser neither
// validates nor sends them.
const kind = document.getElementById("kind");
const sync = () => {
    for (const element of document.querySelectorAll("[data-kind]")) {
        const hidden = element.dataset.kind !== kind.value;
        element.hidden = hidden;
        for (const field of element.querySelectorAll("input, select, textarea")) {
            field.disabled = hidden;
        }
    }
};
kind?.addEventListener("change", sync);
if (kind) sync();
