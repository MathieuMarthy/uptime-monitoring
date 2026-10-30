// Filters the dashboard rows on their name and URL, ignoring case and accents, and hides the groups
// left without a row, unfolding those with matches.
const input = document.getElementById("search-monitors");
const fold = (text) => text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();

function filter() {
    const query = fold(input.value.trim());
    for (const item of document.querySelectorAll("[data-search]")) {
        item.hidden = !fold(item.dataset.search).includes(query);
    }
    for (const group of document.querySelectorAll("[data-group]")) {
        group.hidden = !group.querySelector("[data-search]:not([hidden])");
        if (query && !group.hidden) {
            group.open = true; // a folded group would hide its matches
        }
    }
}

filter(); // the browser may restore the query on back navigation
input.addEventListener("input", filter);
document.addEventListener("live:refreshed", filter);
