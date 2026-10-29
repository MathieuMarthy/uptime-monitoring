// Shows only the fields of the selected channel type. Without JavaScript, all are shown.
(() => {
    const kind = document.getElementById("kind");
    if (!kind) return;
    const sections = document.querySelectorAll("[data-kinds]");
    const update = () => {
        for (const section of sections) {
            section.hidden = !section.dataset.kinds.split(" ").includes(kind.value);
        }
    };
    kind.addEventListener("change", update);
    update();
})();
