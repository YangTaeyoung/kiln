const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const context = { window: {}, document: { readyState: 'loading', addEventListener() {} } };
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../assets/downloads.js'), 'utf8'), context);
const detect = context.window.KilnDownloads.detect;
const cases = [
  ['Windows client hints take priority', { userAgentData: { platform: 'Windows', mobile: false }, platform: 'MacIntel' }, 'windows'],
  ['macOS client hints', { userAgentData: { platform: 'macOS', mobile: false } }, 'mac'],
  ['Safari on macOS without client hints', { platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', maxTouchPoints: 0 }, 'mac'],
  ['Firefox on Windows without client hints', { platform: 'Win32', userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' }, 'windows'],
  ['Windows user agent fallback', { userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' }, 'windows'],
  ['Mac user agent fallback', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' }, 'mac'],
  ['iPad with desktop Mac identity', { platform: 'MacIntel', maxTouchPoints: 5, userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' }, null],
  ['iPhone Safari', { platform: 'iPhone', userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)' }, null],
  ['Android client hints', { userAgentData: { platform: 'Android', mobile: true }, userAgent: 'Mozilla/5.0 (Linux; Android 15)' }, null],
  ['Linux desktop', { platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' }, null],
  ['ChromeOS', { platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; CrOS x86_64)' }, null],
  ['Missing platform information', {}, null]
];
for (const [name, nav, expected] of cases) test(name, () => assert.equal(detect(nav), expected));
