// Asks for confirmation before submitting a form with a `data-confirm` message, or through a
// button carrying one.
document.addEventListener("submit", (event) => {
    const message = event.submitter?.dataset.confirm ?? event.target.dataset.confirm;
    if (message && !window.confirm(message)) {
        event.preventDefault();
    }
});
