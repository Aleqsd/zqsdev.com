import init from "./pkg/zqs_terminal.js";

async function boot() {
    try {
        await init();
    } catch (err) {
        console.error("Failed to bootstrap WebAssembly terminal:", err);
        const fallback = document.getElementById("output");
        if (fallback) {
            fallback.textContent = "Failed to load the terminal. Please refresh the page.";
        }
    }
}

boot();
