// Pre-fills the instance timezone with the browser's; the server falls back to UTC.
const field = document.getElementById("timezone");
const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
if (field && timezone) {
    field.value = timezone;
}
