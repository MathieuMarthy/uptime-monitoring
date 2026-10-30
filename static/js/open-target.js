// Unfolds the <details> a link's #fragment points at (an incident after saving its note): some
// browsers only scroll to its folded summary.
const open = () => {
    const target = location.hash && document.getElementById(decodeURIComponent(location.hash.slice(1)));
    if (target instanceof HTMLDetailsElement) {
        target.open = true;
        target.scrollIntoView();
    }
};
open();
window.addEventListener("hashchange", open);
