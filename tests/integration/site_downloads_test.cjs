// Download links stay on /releases/latest/ until the page resolves the current tag.
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const root = path.join('docs', 'site');
const htmlFiles = [];
function walk(dir) {
  for (const name of fs.readdirSync(dir)) {
    const file = path.join(dir, name);
    if (fs.statSync(file).isDirectory()) walk(file);
    else if (name.endsWith('.html')) htmlFiles.push(file);
  }
}
walk(root);

const hrefs = [];
for (const file of htmlFiles) {
  const text = fs.readFileSync(file, 'utf8');
  for (const match of text.matchAll(/href="([^"]*github\.com\/sockrockets\/sockrocket\/releases[^"]*)"/g)) {
    hrefs.push([file, match[1]]);
    if (match[1].includes('/download/')) {
      assert.match(match[1], /\/releases\/latest\/download\//, file);
    }
    assert.doesNotMatch(match[1], /\/releases\/download\/v\d/, file);
  }
}
assert.ok(hrefs.length >= 10, 'site should publish direct download links');

const home = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
assert.match(home, /data-release-version/);
assert.match(home, /src="release\.js"/);
const routing = fs.readFileSync(path.join(root, 'docs', 'routing.html'), 'utf8');
assert.match(home, /id="later"/);
assert.match(home, /流量统计/);
assert.match(home, /faqFiveQ/);
assert.match(routing, /让分组生效/);
assert.match(routing, /Manual/);
assert.match(routing, /预设、设备与去向检查/);
assert.match(routing, /Device default/);
const merlin = fs.readFileSync(path.join(root, 'docs', 'merlin.html'), 'utf8');
assert.match(merlin, /装好之后，页面上有什么/);
assert.match(merlin, /What the pages do/);
assert.match(merlin, /先刷出软件中心/);
assert.match(merlin, /fw\.koolcenter\.com/);
assert.match(home, /firmwareLead/);

const links = [{ href: 'https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-merlin-hnd_v8.tar.gz' }];
const version = { textContent: 'latest' };
const base = { textContent: '' };
const document = {
  querySelectorAll(selector) {
    if (selector.includes('a[href')) return links;
    if (selector.includes('data-release-version')) return [version];
    if (selector.includes('data-release-base')) return [base];
    return [];
  },
};
const fetchCalls = [];
vm.runInContext(
  fs.readFileSync(path.join(root, 'release.js'), 'utf8'),
  vm.createContext({
    document,
    fetch(url) {
      fetchCalls.push(url);
      return Promise.resolve({ ok: true, json: async () => ({ tag_name: 'v0.0.7' }) });
    },
  }),
);
assert.equal(fetchCalls[0], 'https://api.github.com/repos/sockrockets/sockrocket/releases/latest');
setTimeout(() => {
  assert.equal(links[0].href, 'https://github.com/sockrockets/sockrocket/releases/download/v0.0.7/sockrocket-merlin-hnd_v8.tar.gz');
  assert.equal(version.textContent, 'v0.0.7');
  assert.equal(base.textContent, 'https://github.com/sockrockets/sockrocket/releases/download/v0.0.7/');
  console.log('PASS: site downloads follow the latest release tag');
}, 0);
