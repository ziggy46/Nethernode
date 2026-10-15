// Applies the saved color theme before the page paints, so a light theme
// doesn't flash dark first. src/lib/theme.ts takes over once the app loads.
(() => {
  let choice = "system";
  try {
    choice = localStorage.getItem("nn.theme") || "system";
  } catch {
    // Storage blocked: follow the device.
  }
  const light =
    choice === "light" || (choice !== "dark" && window.matchMedia?.("(prefers-color-scheme: light)").matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
})();
