// Copies the text of the element named by a button's `data-copy`, then says so on the button
// with its `data-copied` or `data-copy-failed` text.
document.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-copy]");
    const source = button && document.getElementById(button.dataset.copy);
    if (!source) return;
    const label = button.dataset.label ?? button.textContent;
    button.dataset.label = label;
    try {
        await navigator.clipboard.writeText(source.textContent.trim());
        button.textContent = button.dataset.copied;
    } catch {
        button.textContent = button.dataset.copyFailed;
    }
    setTimeout(() => { button.textContent = label; }, 2000);
});
