// A filter form marked `data-auto-submit` applies a new choice at once: its Show button is only
// needed without JavaScript.
for (const form of document.querySelectorAll("form[data-auto-submit]")) {
    form.querySelector("[data-submit]").hidden = true;
    form.addEventListener("change", () => form.requestSubmit());
}
