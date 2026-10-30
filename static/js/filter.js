// Filters the dashboard rows at once, as the server does from `/?q=&status=&group=`: the search
// on name, address and group (ignoring case and accents), the state chips and the group menu. The
// URL follows, so the live refresh, a reload and a shared link show the same rows. Groups left
// without a row are hidden, those with search matches unfolded. `/` focuses the search, Esc clears it.
const form = document.getElementById("filter-form");

if (form) {
    const input = document.getElementById("search-monitors");
    const select = document.getElementById("filter-group");
    const noMatch = document.querySelector("[data-no-match]");
    const range = new URLSearchParams(location.search).get("range") === "90d" ? "90d" : "";
    const fold = (text) => text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
    let status = new URLSearchParams(location.search).get("status") ?? "";
    if (!["down", "paused"].includes(status)) {
        status = "";
    }

    // The dashboard URL with the current filter, some values replaced.
    const url = (next = {}) => {
        const values = { range, q: input.value.trim(), status, group: select?.value ?? "", ...next };
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
        const group = select?.value ?? "";
        for (const item of document.querySelectorAll("li[data-search]")) {
            item.hidden =
                !fold(item.dataset.search).includes(query) || (status !== "" && item.dataset.status !== status);
        }
        for (const details of document.querySelectorAll("details[data-group]")) {
            details.hidden =
                (group !== "" && details.dataset.group !== group) ||
                !details.querySelector("li[data-search]:not([hidden])");
            if (query && !details.hidden) {
                details.open = true; // a folded group would hide its matches
            }
        }
        const matches = [...document.querySelectorAll("li[data-search]:not([hidden])")].some(
            (item) => !item.closest("details[hidden]"),
        );
        noMatch.hidden = matches;

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
    select?.addEventListener("change", filter);
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
        if (!chip && !reset) {
            return;
        }
        event.preventDefault();
        if (chip) {
            status = chip.dataset.status;
        } else {
            status = "";
            input.value = "";
            if (select) {
                select.value = "";
            }
        }
        filter();
    });

    filter(); // the browser may restore the fields on back navigation
    document.addEventListener("live:refreshed", filter);
}
