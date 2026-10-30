// Filters the dashboard rows at once, as the server does from `/?q=&status=&group=`: the search
// on name, address and group (ignoring case and accents), the state chips and the group menu. The
// URL follows, so the live refresh, a reload and a shared link show the same rows. Groups left
// without a row are hidden, those with search matches unfolded. `/` focuses the search, Esc clears it.
const form = document.getElementById("filter-form");

if (form) {
    const input = document.getElementById("search-monitors");
    const noMatch = document.querySelector("[data-no-match]");
    const params = new URLSearchParams(location.search);
    const range = params.get("range") === "90d" ? "90d" : "";
    // As the server: without combining marks, and group names compared like `name_key`.
    const fold = (text) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
    const key = (name) => name.toLowerCase().normalize("NFC");
    let status = ["down", "paused"].includes(params.get("status")) ? params.get("status") : "";
    // Read from the URL, not the menu: the menu is missing when no monitor has a group.
    let group = params.get("group")?.trim() ?? "";
    const groupOf = (item) => item.closest("details[data-group]")?.dataset.group ?? "Ungrouped";

    // The dashboard URL with the current filter, some values replaced.
    const url = (next = {}) => {
        const values = { range, q: input.value.trim(), status, group, ...next };
        const params = new URLSearchParams();
        for (const [name, value] of Object.entries(values)) {
            if (value) {
                params.set(name, value);
            }
        }
        return params.size ? `/?${params}` : "/";
    };

    const filter = () => {
        const query = fold(input.value.trim());
        for (const item of document.querySelectorAll("li[data-search]")) {
            item.hidden =
                !fold(item.dataset.search).includes(query) ||
                (status !== "" && item.dataset.status !== status) ||
                (group !== "" && key(groupOf(item)) !== key(group));
        }
        for (const details of document.querySelectorAll("details[data-group]")) {
            details.hidden = !details.querySelector("li[data-search]:not([hidden])");
            if (query && !details.hidden) {
                details.open = true; // a folded group would hide its matches
            }
        }
        noMatch.hidden = !document.querySelector("li[data-search]:not([hidden])");

        for (const chip of document.querySelectorAll("#status-filter a")) {
            chip.href = url({ status: chip.dataset.status });
            if (chip.dataset.status === status) {
                chip.setAttribute("aria-current", "true");
            } else {
                chip.removeAttribute("aria-current");
            }
        }
        for (const link of document.querySelectorAll("a[data-range]")) {
            link.href = url({ range: link.dataset.range });
        }
        noMatch.querySelector("[data-reset]").href = url({ q: "", status: "", group: "" });
        history.replaceState(history.state, "", url());
    };

    form.querySelector("[data-submit]").hidden = true;
    form.addEventListener("submit", (event) => {
        event.preventDefault();
        filter();
    });
    input.addEventListener("input", filter);
    // The group menu is swapped by the live refresh: listen above it.
    document.addEventListener("change", (event) => {
        if (event.target.id === "filter-group") {
            group = event.target.value;
            filter();
        }
    });
    input.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && input.value) {
            input.value = "";
            filter();
        }
    });
    document.addEventListener("keydown", (event) => {
        const typing = event.target.closest("input, textarea, select, [contenteditable]");
        if (event.key === "/" && !typing && !event.ctrlKey && !event.metaKey && !event.altKey) {
            event.preventDefault();
            input.focus();
            input.select();
        }
    });
    // Chips are swapped by the live refresh: listen above them.
    document.addEventListener("click", (event) => {
        const chip = event.target.closest("#status-filter a");
        const reset = event.target.closest("[data-reset]");
        const newTab = event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey;
        if ((!chip && !reset) || newTab) {
            return;
        }
        event.preventDefault();
        if (chip) {
            status = chip.dataset.status;
        } else {
            status = "";
            group = "";
            input.value = "";
            const select = document.getElementById("filter-group");
            if (select) {
                select.value = "";
            }
        }
        filter();
    });

    filter(); // the browser may restore the fields on back navigation
    document.addEventListener("live:refreshed", filter);
}
