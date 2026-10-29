// Applies the theme picked with the header's switch before the page is drawn (loaded without
// defer in <head>), then wires the switch: system, light, dark, and round again. The choice is
// kept in this browser only; without it the system theme decides.
const THEME_KEY = "theme";
const THEMES = ["system", "light", "dark"];

function storedTheme() {
    try {
        return localStorage.getItem(THEME_KEY) ?? "system";
    } catch {
        return "system"; // storage blocked: follow the system
    }
}

function applyTheme(theme) {
    if (theme === "light" || theme === "dark") {
        document.documentElement.dataset.theme = theme;
    } else {
        delete document.documentElement.dataset.theme;
    }
}

applyTheme(storedTheme());

document.addEventListener("DOMContentLoaded", () => {
    const button = document.getElementById("theme-switch");
    if (!button) {
        return;
    }
    const label = button.querySelector("[data-theme-label]");
    const show = (theme) => {
        button.dataset.mode = theme;
        label.textContent = theme[0].toUpperCase() + theme.slice(1);
    };
    show(storedTheme());
    button.hidden = false; // useless without this script
    button.addEventListener("click", () => {
        const next = THEMES[(THEMES.indexOf(button.dataset.mode) + 1) % THEMES.length];
        try {
            localStorage.setItem(THEME_KEY, next);
        } catch {
            // storage blocked: the choice lasts until the page is left
        }
        applyTheme(next);
        show(next);
    });
});
