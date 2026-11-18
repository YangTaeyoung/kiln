import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const source = await readFile(new URL('../crates/kiln/src/app/agent_gui/native_return.js', import.meta.url), 'utf8');
const { register } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));

async function fixture() {
  const hooks = new Map();
  register((name, ...args) => hooks.set(name, args.at(-1)));
  let timer, now = 1000, text = '', exits = 0;
  const files = new Map();
  const $ = {
    plugin: { root: '/owned' }, env: { get: async () => 'launch' }, session: { id: async () => 'session' },
    command: { list: async () => [{ name: 'exit', source: 'builtin' }], run: async () => { exits++; } },
    clock: { now: () => now, every: (_, callback) => { timer = callback; } },
    prompt: { read: async () => ({ text }) }, ui: { invalidate() {} },
    fs: { write: async (p, value) => files.set(p, value), exists: async p => files.has(p), read: async p => files.get(p) },
  };
  await hooks.get('session.start')($, {}, async () => ({}));
  files.set('/owned/return.request', JSON.stringify({ protocol: 1, token: 'launch', session: 'session' }));
  return {
    async poll(props, age = 0) {
      if (props !== undefined) hooks.get('ui.render')($, { surface: 'terminal', props }, event => event);
      now += age;
      await timer();
      return { exits, phase: JSON.parse(files.get('/owned/bridge.state')).phase };
    },
    draft(value) { text = value; },
    request(value) { files.set('/owned/return.request', JSON.stringify(value)); },
  };
}
const idle = { isWorking: false, isDraft: false, hint: '' };

test('only a fresh, fully reported idle prompt can exit', async () => {
  for (const [props, age] of [[{}, 0], [{ ...idle, isWorking: undefined }, 0], [idle, 751]]) {
    const f = await fixture();
    assert.equal((await f.poll(props, age)).exits, 0);
  }
  assert.deepEqual(await (await fixture()).poll(idle), { exits: 1, phase: 'closing' });
});

test('work, native draft and background tasks defer return', async () => {
  for (const props of [{ ...idle, isWorking: true }, { ...idle, isDraft: true }, { ...idle, hint: '1 shell · 1 monitor · ↓ to manage' }]) {
    const f = await fixture();
    assert.equal((await f.poll(props)).exits, 0);
    assert.equal((await f.poll(idle)).exits, 1);
  }
  const f = await fixture();
  f.draft('unsubmitted text');
  assert.equal((await f.poll(idle)).exits, 0);
});

test('another launch or conversation cannot request an exit', async () => {
  for (const request of [{ protocol: 1, token: 'old', session: 'session' }, { protocol: 1, token: 'launch', session: 'other' }]) {
    const f = await fixture();
    f.request(request);
    assert.equal((await f.poll(idle)).exits, 0);
  }
});
