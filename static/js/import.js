// Loads the chosen configuration file into the import form's text area.
(() => {
    const file = document.getElementById("import-file");
    const text = document.getElementById("document");
    if (!file || !text) return;
    file.addEventListener("change", async () => {
        const chosen = file.files[0];
        if (chosen) text.value = await chosen.text();
    });
})();
