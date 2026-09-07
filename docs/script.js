const REPO = "codenameakshay/wtm-manager";
const RAW_ASSETS = "https://raw.githubusercontent.com/" + REPO + "/main/assets/";
const LATEST = "https://github.com/" + REPO + "/releases/latest/download/";

const demos = {
  tui: { title: "Browse in the TUI", image: "tui.gif", alt: "wtm's full-screen TUI listing worktrees with status pills" },
  list: { title: "Inspect the registry", image: "list.gif", alt: "wtm list printing worktree paths and status badges" },
  add: { title: "Create with setup", image: "add.gif", alt: "wtm add creating a worktree and running setup commands" },
  switch: { title: "Switch context", image: "switch.gif", alt: "wtm switch changing the shell's directory to a worktree" },
  prune: { title: "Prune with a preview", image: "prune.gif", alt: "wtm prune previewing merged and upstream-gone worktrees" }
};

function setDemo(name) {
  const demo = demos[name];
  if (!demo) return;
  document.querySelector("#demo-title").textContent = demo.title;
  const image = document.querySelector("#demo-image");
  image.src = RAW_ASSETS + demo.image;
  image.alt = demo.alt;
  document.querySelectorAll("[data-demo]").forEach((button) => {
    const active = button.dataset.demo === name;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  });
  document.querySelector("#demo-status").textContent = demo.title + " recording selected";
}
document.querySelectorAll("[data-demo]").forEach((button) => {
  button.addEventListener("click", () => setDemo(button.dataset.demo));
});

function formatBytes(bytes) {
  if (!Number.isFinite(bytes)) return "";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return value.toFixed(value >= 10 || unit === 0 ? 0 : 1) + " " + units[unit];
}

// Every download link on the page is a stable releases/latest/download URL,
// so the page works without this request; the release API only fills in
// sizes and the version line.
async function loadLatestRelease() {
  const status = document.querySelector("#release-status");
  try {
    const response = await fetch("https://api.github.com/repos/" + REPO + "/releases/latest", {
      headers: { Accept: "application/vnd.github+json" }
    });
    if (!response.ok) throw new Error("release request failed");
    const release = await response.json();
    const assets = Array.isArray(release.assets) ? release.assets : [];
    const when = release.published_at ? new Date(release.published_at).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" }) : "";
    status.textContent = (release.tag_name || "Latest release") + (when ? ", released " + when : "");
    document.querySelector(".release-dot").classList.add("is-live");
    if (release.html_url) document.querySelector("#release-page").href = release.html_url;
    document.querySelectorAll("[data-asset]").forEach((cell) => {
      const asset = assets.find((a) => a.name === cell.dataset.asset);
      if (asset) cell.textContent = formatBytes(asset.size);
    });
  } catch (error) {
    status.textContent = "Release details are on GitHub";
  }
}

// A Linux visitor gets the .deb as the primary download; there is no
// reliable ARM detection in browsers, so the x86_64 build is offered and
// the ARM64 row is one scroll away.
function applyPlatformDefaults() {
  try {
    const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "";
    if (!/linux/i.test(platform) || /android/i.test(navigator.userAgent)) return;
    document.querySelectorAll("[data-download-primary]").forEach((link) => {
      link.href = LATEST + "WTM-linux-x86_64.deb";
      link.textContent = "Download for Linux";
      link.title = "x86_64 .deb; the ARM64 build is in the download table";
    });
    document.querySelector("#hero-note").textContent = "x86_64 .deb. ARM64 and the tarball are in the download table.";
    document.querySelector('[data-platform="macos"]').classList.remove("is-recommended");
    document.querySelector('[data-platform="linux-x64"]').classList.add("is-recommended");
  } catch (error) {
    // Leave the macOS defaults in place.
  }
}

loadLatestRelease();
applyPlatformDefaults();
