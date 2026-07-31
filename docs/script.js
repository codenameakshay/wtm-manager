const REPO = "codenameakshay/wtm-manager";
const RAW_ASSETS = "https://raw.githubusercontent.com/" + REPO + "/main/assets/";
const RELEASE_PAGE = "https://github.com/" + REPO + "/releases/latest";

const demos = {
  tui: {
    title: "Full-screen TUI",
    image: RAW_ASSETS + "tui.gif",
    alt: "wtm full-screen TUI showing worktrees and status badges"
  },
  list: {
    title: "Inspect the registry",
    image: RAW_ASSETS + "list.gif",
    alt: "wtm list showing worktree paths and status badges"
  },
  add: {
    title: "Create with setup",
    image: RAW_ASSETS + "add.gif",
    alt: "wtm add creating a new worktree and running setup automation"
  },
  switch: {
    title: "Switch context",
    image: RAW_ASSETS + "switch.gif",
    alt: "wtm switch changing into a worktree through the shell wrapper"
  },
  prune: {
    title: "Clean with confidence",
    image: RAW_ASSETS + "prune.gif",
    alt: "wtm prune previewing merged and upstream-gone worktrees"
  }
};

const platformMatchers = {
  "macos-arm64": ["aarch64-apple-darwin"],
  "macos-intel": ["x86_64-apple-darwin"],
  "linux-x64": ["x86_64-unknown-linux-gnu"],
  "linux-arm64": ["aarch64-unknown-linux-gnu"]
};

const fallbackRelease = "https://github.com/" + REPO + "/releases/latest";

function setDemo(name) {
  const demo = demos[name];
  if (!demo) return;
  document.querySelector("#demo-title").textContent = demo.title;
  const image = document.querySelector("#demo-image");
  image.src = demo.image;
  image.alt = demo.alt;
  document.querySelector("#demo-open").href = demo.image;
  document.querySelectorAll("[data-demo]").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.demo === name);
  });
}

document.querySelectorAll("[data-demo]").forEach((button) => {
  button.addEventListener("click", () => setDemo(button.dataset.demo));
});

function formatBytes(bytes) {
  if (!Number.isFinite(bytes)) return "Release asset";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return value.toFixed(value >= 10 || unit === 0 ? 0 : 1) + " " + units[unit];
}

function matchingAsset(assets, patterns) {
  return assets.find((asset) => patterns.some((pattern) => asset.name.includes(pattern)));
}

function setFallbackLinks() {
  document.querySelectorAll("[data-platform] .download-link").forEach((link) => {
    link.href = fallbackRelease;
    link.textContent = "View latest build ↗";
  });
  document.querySelectorAll("[data-platform] .asset-meta").forEach((meta) => {
    meta.textContent = "See release assets";
  });
}

async function loadLatestRelease() {
  const status = document.querySelector("#release-status");
  const releasePage = document.querySelector("#release-page");
  setFallbackLinks();

  try {
    const response = await fetch("https://api.github.com/repos/" + REPO + "/releases/latest", {
      headers: { Accept: "application/vnd.github+json" }
    });
    if (!response.ok) throw new Error("release request failed");
    const release = await response.json();
    const assets = Array.isArray(release.assets) ? release.assets : [];

    status.textContent = (release.tag_name || "Latest release") + (release.published_at ? " · " + new Date(release.published_at).toLocaleDateString() : "");
    const releaseDot = document.querySelector(".release-dot");
    if (releaseDot) releaseDot.style.background = "var(--green)";
    releasePage.href = release.html_url || fallbackRelease;

    Object.entries(platformMatchers).forEach(([platform, patterns]) => {
      const card = document.querySelector('[data-platform="' + platform + '"]');
      if (!card) return;
      const link = card.querySelector(".download-link");
      const meta = card.querySelector(".asset-meta");
      const asset = matchingAsset(assets, patterns);
      if (asset) {
        link.href = asset.browser_download_url;
        link.textContent = "Download build ↗";
        meta.textContent = formatBytes(asset.size) + " · " + asset.name;
      }
    });

    const installer = assets.find((asset) => /installer\.sh$/i.test(asset.name));
    document.querySelectorAll("[data-installer]").forEach((link) => {
      link.href = installer ? installer.browser_download_url : fallbackRelease;
      link.textContent = installer ? "Download installer ↗" : "View installer assets ↗";
    });
  } catch (error) {
    status.textContent = "Release links available on GitHub";
    releasePage.href = fallbackRelease;
  }
}

loadLatestRelease();
