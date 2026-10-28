// The session cookie is `Secure`: browsers drop it over plain HTTP, except on localhost.
const local = ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);
if (location.protocol === "http:" && !local) {
    document.getElementById("insecure-warning")?.removeAttribute("hidden");
}
