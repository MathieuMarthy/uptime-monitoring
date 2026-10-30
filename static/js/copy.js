// Copies the text of the element named by a button's `data-copy`, then says so on the button.
document.addEventListener("click", async (event) => {
    const button = event.target.closest("[data-copy]");
    const source = button && document.getElementById(button.dataset.copy);
    if (!source) return;
    try {
        await navigator.clipboard.writeText(source.textContent.trim());
        button.textContent = "Copied";
    } catch {
        button.textContent = "Copy failed";
    }
    setTimeout(() => { button.textContent = "Copy"; }, 2000);
});
