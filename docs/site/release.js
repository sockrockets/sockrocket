'use strict';

const RELEASE_LATEST = '/releases/latest/download/';

function applyRelease(tag) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag)) return;
  const pinned = '/releases/download/' + tag + '/';
  document.querySelectorAll('a[href*="' + RELEASE_LATEST + '"]').forEach((link) => {
    link.href = link.href.replace(RELEASE_LATEST, pinned);
  });
  document.querySelectorAll('[data-release-version]').forEach((node) => {
    node.textContent = tag;
  });
  document.querySelectorAll('[data-release-base]').forEach((node) => {
    node.textContent = 'https://github.com/sockrockets/sockrocket' + pinned;
  });
}

fetch('https://api.github.com/repos/sockrockets/sockrocket/releases/latest', {
  headers: { Accept: 'application/vnd.github+json' },
})
  .then((response) => (response.ok ? response.json() : null))
  .then((release) => {
    if (release && release.tag_name) applyRelease(release.tag_name);
  })
  .catch(() => {});
