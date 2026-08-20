(() => {
  const repositoryMeta = document.querySelector('meta[name="github-repository"]');
  const status = document.querySelector('#release-status');
  const releasePage = document.querySelector('#release-page');

  function githubRepository() {
    const configured = repositoryMeta?.content.trim();
    if (configured && /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(configured)) {
      return configured;
    }

    const pagesHost = window.location.hostname.match(/^([A-Za-z0-9-]+)\.github\.io$/i);
    if (!pagesHost) return null;

    const owner = pagesHost[1];
    const firstPathPart = window.location.pathname.split('/').filter(Boolean)[0];
    const projectPage = firstPathPart && !/\.html?$/i.test(firstPathPart);
    return `${owner}/${projectPage ? decodeURIComponent(firstPathPart) : `${owner}.github.io`}`;
  }

  function validAsset(asset) {
    if (!asset || typeof asset.name !== 'string' || typeof asset.browser_download_url !== 'string') {
      return false;
    }
    try {
      const url = new URL(asset.browser_download_url);
      return url.protocol === 'https:' && url.hostname === 'github.com';
    } catch {
      return false;
    }
  }

  function matchAssets(assets) {
    const safeAssets = assets.filter(validAsset);
    const dmgs = safeAssets.filter((asset) => /\.dmg$/i.test(asset.name));
    const macArm = dmgs.find((asset) => /(aarch64|arm64|apple[-_ ]?silicon)/i.test(asset.name));
    const macIntel = dmgs.find((asset) => /(x64|x86_64|amd64|intel)/i.test(asset.name));

    return {
      windows: safeAssets.find((asset) => /\.exe$/i.test(asset.name)),
      'mac-arm': macArm || (dmgs.length === 1 ? dmgs[0] : null),
      'mac-intel': macIntel || (dmgs.length === 1 ? dmgs[0] : null),
      linux: safeAssets.find((asset) => /\.appimage$/i.test(asset.name))
    };
  }

  function enableLinks(platform, asset) {
    if (!asset) return false;
    document.querySelectorAll(`[data-release-platform="${platform}"]`).forEach((link) => {
      link.href = asset.browser_download_url;
      link.removeAttribute('aria-disabled');
      link.title = `${asset.name} — ${(asset.size / 1024 / 1024).toFixed(1)} MB`;
    });
    return true;
  }

  async function loadLatestRelease() {
    const repository = githubRepository();
    if (!repository) {
      status.textContent = 'Set the github-repository meta value when using a custom domain.';
      return;
    }

    const releasesUrl = `https://github.com/${repository}/releases/latest`;
    releasePage.href = releasesUrl;
    releasePage.hidden = false;
    document.querySelectorAll('[data-release-page]').forEach((link) => {
      link.href = releasesUrl;
      link.removeAttribute('aria-disabled');
    });

    try {
      const response = await fetch(`https://api.github.com/repos/${repository}/releases/latest`, {
        headers: { Accept: 'application/vnd.github+json' }
      });
      if (!response.ok) throw new Error(`GitHub returned ${response.status}`);

      const release = await response.json();
      const version = String(release.tag_name || release.name || 'latest').replace(/^v/i, '');
      if (typeof release.html_url === 'string') {
        releasePage.href = release.html_url;
        document.querySelectorAll('[data-release-page]').forEach((link) => {
          link.href = release.html_url;
        });
      }
      document.querySelectorAll('[data-release-version]').forEach((element) => {
        element.textContent = version;
      });

      const matched = matchAssets(Array.isArray(release.assets) ? release.assets : []);
      const available = Object.entries(matched)
        .filter(([platform, asset]) => enableLinks(platform, asset))
        .map(([platform]) => platform);

      if (available.length === 4) {
        status.textContent = `Napstr ${version} downloads are ready.`;
      } else {
        status.textContent = `Napstr ${version} is published, but one or more installers are still uploading.`;
      }
    } catch (error) {
      status.textContent = 'The automatic download list is temporarily unavailable.';
      console.warn('Could not load the latest Napstr release:', error);
    }
  }

  loadLatestRelease();
})();
