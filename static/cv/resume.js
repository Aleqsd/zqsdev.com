// The resume and PDF link are usable before this optional enhancement loads.
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
      showToast("Email copied to clipboard");
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
if (fromTerminal) cta?.remove();
