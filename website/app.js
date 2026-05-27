const switcher = document.querySelector(".mode-switch");
const buttons = [...document.querySelectorAll(".mode-switch button")];
const previews = [...document.querySelectorAll("[data-preview]")];

switcher?.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-mode]");
  if (!button) return;

  const mode = button.dataset.mode;
  for (const item of buttons) {
    const active = item === button;
    item.classList.toggle("active", active);
    item.setAttribute("aria-selected", String(active));
  }
  for (const preview of previews) {
    preview.classList.toggle("active", preview.dataset.preview === mode);
  }
});
