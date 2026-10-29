// Recomputes the disk estimates of the data settings while typing (same figures as views/settings.rs).
(() => {
    const CHECK_BYTES = 32;
    const DAILY_STAT_BYTES = 36;

    const formatBytes = (bytes) => {
        if (bytes < 1024) return `${Math.round(bytes)} B`;
        const units = ["KiB", "MiB", "GiB", "TiB"];
        let value = bytes / 1024;
        let unit = 0;
        while (value >= 1024 && unit < units.length - 1) {
            value /= 1024;
            unit++;
        }
        return value < 10 ? `${value.toFixed(1)} ${units[unit]}` : `${Math.round(value)} ${units[unit]}`;
    };

    const form = document.getElementById("data-settings");
    if (!form) return;
    const checksPerDay = Number(form.dataset.checksPerDay);
    const perDayOfHistory = Number(form.dataset.monitors) * DAILY_STAT_BYTES;
    const checksDays = document.getElementById("checks_retention_days");
    const historyDays = document.getElementById("history_retention_days");

    const update = () => {
        const checks = Number(checksDays.value);
        document.getElementById("checks-estimate").textContent =
            checksDays.value.trim() && checks > 0 ? formatBytes(checksPerDay * checks * CHECK_BYTES) : "—";
        const history = Number(historyDays.value);
        document.getElementById("history-estimate").textContent = !historyDays.value.trim()
            ? `${formatBytes(perDayOfHistory * 365)} per year, kept forever`
            : history > 0 ? formatBytes(perDayOfHistory * history) : "—";
    };
    checksDays.addEventListener("input", update);
    historyDays.addEventListener("input", update);
})();
