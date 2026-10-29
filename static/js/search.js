// Filters the dashboard rows on their name and URL, ignoring case and accents.
const input = document.getElementById("search-monitors");
const fold = (text) => text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();

function filter() {
    const query = fold(input.value.trim());
    for (const item of document.querySelectorAll("[data-search]")) {
        item.hidden = !fold(item.dataset.search).includes(query);
    }
}

filter(); // the browser may restore the query on back navigation
input.addEventListener("input", filter);
document.addEventListener("live:refreshed", filter);
