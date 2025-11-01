// PDF.js (ESM via CDN)
import {
  getDocument,
  GlobalWorkerOptions,
} from "https://cdn.jsdelivr.net/npm/pdfjs-dist@5.4.54/build/pdf.mjs";
GlobalWorkerOptions.workerSrc =
  "https://cdn.jsdelivr.net/npm/pdfjs-dist@5.4.54/build/pdf.worker.mjs";

const PDF_URL = "CV_Alexandre_DO_O_ALMEIDA_2025.pdf?v=7";
const viewer = document.getElementById("viewer");
const loading = document.getElementById("loading");

const state = { pdf: null, pages: [], scaleByWidth: true };

function clearViewer() {
  viewer.querySelectorAll(".page").forEach((el) => el.remove());
}

async function renderPage(page, canvas, scale) {
  const viewport = page.getViewport({ scale });
  const ctx = canvas.getContext("2d", { alpha: false });

  const outputScale = window.devicePixelRatio || 1;
  canvas.width = Math.floor(viewport.width * outputScale);
  canvas.height = Math.floor(viewport.height * outputScale);

  canvas.style.width = `${Math.floor(viewport.width)}px`;
  canvas.style.height = `${Math.floor(viewport.height)}px`;

  const transform =
    outputScale !== 1 ? [outputScale, 0, 0, outputScale, 0, 0] : null;

  await page.render({ canvasContext: ctx, viewport, transform }).promise;
}

function computeScale(page, containerWidth, padding = 16) {
  const unscaled = page.getViewport({ scale: 1 });
  const avail = containerWidth - padding * 2;
  return Math.max(0.1, avail / unscaled.width);
}

async function renderAllPages() {
  if (!state.pdf) return;
  clearViewer();
  const containerWidth = viewer.clientWidth;

  for (let num = 1; num <= state.pdf.numPages; num++) {
    const page = await state.pdf.getPage(num);

    const wrapper = document.createElement("div");
    wrapper.className = "page";

    const link = document.createElement("a");
    link.href = PDF_URL;
    link.target = "_blank";
    link.rel = "noopener";

    const canvas = document.createElement("canvas");
    link.appendChild(canvas);
    wrapper.appendChild(link);
    viewer.appendChild(wrapper);

    const scale = computeScale(page, containerWidth);
    await renderPage(page, canvas, scale);
    state.pages[num] = { page, canvas, scale };
  }
}

// Resize (debounced via rAF)
let resizeTimer = null;
window.addEventListener("resize", () => {
  if (resizeTimer) cancelAnimationFrame(resizeTimer);
  resizeTimer = requestAnimationFrame(async () => {
    const containerWidth = viewer.clientWidth;
    for (let num = 1; num <= (state.pdf?.numPages || 0); num++) {
      const entry = state.pages[num];
      if (!entry) continue;
      const newScale = computeScale(entry.page, containerWidth);
      if (Math.abs(newScale - entry.scale) > 0.02) {
        await renderPage(entry.page, entry.canvas, newScale);
        entry.scale = newScale;
      }
    }
  });
});

function showToast(message) {
  const toast = document.getElementById("toast");
  if (!toast) return;
  toast.innerHTML = `<span class="icon">📋</span>${message}`;
  toast.classList.add("show");
  clearTimeout(showToast._t);
  showToast._t = setTimeout(() => toast.classList.remove("show"), 1800);
}

// Copy email + popup
const emailBtn = document.getElementById("copy-email");
emailBtn.addEventListener("click", async (e) => {
  e.preventDefault();
  const email = "alexandre@zqsdev.com";
  try {
    await navigator.clipboard.writeText(email);
    showToast("Email copied to clipboard");
  } catch {
    // Fallback (sélection dans un input temporaire)
    const tmp = document.createElement("input");
    tmp.value = email;
    document.body.appendChild(tmp);
    tmp.select();
    document.execCommand("copy");
    tmp.remove();
    showToast("Email copied");
  }
});

// Init
(async () => {
  try {
    const task = getDocument(PDF_URL);
    state.pdf = await task.promise;
    await renderAllPages();
  } catch (e) {
    console.error(e);
    viewer.innerHTML = `
      <div class="fallback">
        <p>Unable to display the resume inline.</p>
        <a href="${PDF_URL}" download>Download resume</a>
        <a href="${PDF_URL}" target="_blank" rel="noopener">Open in new tab</a>
      </div>`;
  } finally {
    loading?.remove();
  }
})();
