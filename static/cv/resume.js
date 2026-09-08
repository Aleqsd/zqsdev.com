// The resume and PDF link are usable before this optional enhancement loads.
const isFrench = document.documentElement.lang === "fr";
const email = "alexandre@zqsdev.com";
const emailButton = document.getElementById("copy-email");
const toast = document.getElementById("toast");
let toastTimeout;

function showToast(message) {
  toast.textContent = message;
  toast.classList.add("show");
  clearTimeout(toastTimeout);
  toastTimeout = setTimeout(() => toast.classList.remove("show"), 3000);
}

if (emailButton) {
  emailButton.hidden = false;
  emailButton.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(email);
      showToast(isFrench ? "E-mail copié dans le presse-papiers" : "Email copied to clipboard");
    } catch {
      // Keep the address available even if clipboard permission is denied.
      showToast(email);
    }
  });
}

const cta = document.querySelector(".terminal-cta");
const from = new URL(window.location.href).searchParams.get("from");
let fromTerminal = from?.toLowerCase() === "interactive";
if (document.referrer) {
  try {
    fromTerminal ||= ["zqsdev.com", "www.zqsdev.com"].includes(
      new URL(document.referrer).hostname.toLowerCase(),
    );
  } catch {
    // A missing or malformed referrer should not affect the resume.
  }
}
if (fromTerminal) {
  cta?.remove();
  // Keep the terminal return context across a language change.
  const languageLink = document.querySelector(".language-switch");
  if (languageLink) {
    const target = new URL(languageLink.href);
    target.searchParams.set("from", "interactive");
    languageLink.href = target.href;
  }
}

// Fit the A4 HTML to the original PDF frame on desktop, keeping real text.
const documentElement = document.getElementById("resume");
const frame = documentElement?.parentElement;
if (frame && "ResizeObserver" in window) {
  const fitDocument = () => {
    const desktop = window.matchMedia("(min-width: 841px)").matches;
    documentElement.classList.toggle("fit-document", desktop);
    if (desktop) {
      const frameStyle = getComputedStyle(frame);
      const width = frame.clientWidth - parseFloat(frameStyle.paddingLeft) - parseFloat(frameStyle.paddingRight);
      documentElement.style.setProperty("--resume-zoom", width / (210 * 96 / 25.4));
    } else {
      documentElement.style.removeProperty("--resume-zoom");
    }
  };
  new ResizeObserver(fitDocument).observe(frame);
  window.addEventListener("resize", fitDocument);
  fitDocument();
}
