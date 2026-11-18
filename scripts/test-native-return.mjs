import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const source = await readFile(new URL('../crates/kiln/src/app/agent_gui/native_return.js', import.meta.url), 'utf8');
const { register } = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));

async function fixture({ late = false, target = 'session', callerPid = 7, callerStamp = 'birth', beforePrompt, beforeClosing } = {}) {
  const hooks = new Map();
  register((name, ...args) => hooks.set(name, args.at(-1)));
  const timers=[]; let now = 1000, text = '', exits = 0;
  const files = new Map();
  const $ = {
    plugin: { root: '/owned' }, env: { get: async () => late ? undefined : 'launch' }, session: { id: async () => 'session', messages: async () => [] },
    process: { run: async () => ({exitCode: 0, stdout: JSON.stringify({protocol: 1, pid: callerPid, stamp: callerStamp})}) },
    command: { list: async () => [{ name: 'exit', source: 'builtin' }], run: async () => { exits++; } },
    clock: { now: () => now, every: (_, callback) => { if (!timers.includes(callback)) timers.push(callback); } },
    prompt: { read: async () => { if (beforePrompt) await beforePrompt({files, advance(ms) { now += ms; }}); return {text}; } }, ui: { invalidate() {} },
    fs: { write: async (p, value) => { files.set(p, value); if (beforeClosing && p.endsWith('bridge.state') && JSON.parse(value).phase === 'closing') await beforeClosing(files); }, exists: async p => files.has(p), read: async p => files.get(p) },
  };
  if (late) files.set('/owned/bridge.config', JSON.stringify({ protocol: 1, token: 'launch', session: target, helper: '/owned/kiln', pid: 7, stamp: 'birth' }));
  else await hooks.get('session.start')($, {}, async () => ({}));
  files.set('/owned/return.request', JSON.stringify({ protocol: 1, token: 'launch', session: 'session' }));
  return {
    async poll(props, age = 0) {
      if (props !== undefined) await hooks.get('ui.render')($, { surface: 'terminal', props }, event => event);
      now += age;
      for (let i=0;i<timers.length;i++) await timers[i]();
      return { exits, phase: files.has('/owned/bridge.state') ? JSON.parse(files.get('/owned/bridge.state')).phase : 'unavailable' };
    },
    async start() { await hooks.get('session.start')($, {}, async () => ({})); },
    state() { return files.has('/owned/bridge.state') ? JSON.parse(files.get('/owned/bridge.state')) : undefined; },
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

test('a reloaded bridge can return an already-running CLI without session.start or launch environment', async () => {
  const f = await fixture({ late: true });
  assert.deepEqual(await f.poll({ ...idle, isWorking: true }), { exits: 0, phase: 'working' });
  f.draft('preserved native draft');
  assert.equal((await f.poll(idle)).exits, 0);
  f.draft('');
  assert.deepEqual(await f.poll(idle), { exits: 1, phase: 'closing' });
});

test('a bridge installed in a shared profile leaves every other conversation alone', async () => {
  const f = await fixture({ late: true, target: 'other-session' });
  assert.deepEqual(await f.poll(idle), { exits: 0, phase: 'unavailable' });
});

test('an expired GUI adoption lease never exits its existing CLI', async () => {
  for (const expiresAt of [999, null, 'future']) {
    const f = await fixture({ late: true });
    f.request({ protocol: 1, token: 'launch', session: 'session', expiresAt });
    assert.equal((await f.poll(idle)).exits, 0);
  }
  const f = await fixture({ late: true });
  f.request({ protocol: 1, token: 'launch', session: 'session', expiresAt: 2000 });
  assert.equal((await f.poll(idle)).exits, 1);
});


test('a resumed JSON backend cannot activate the terminal adoption mod', async () => {
  const f = await fixture({ late: true });
  await f.start();
  assert.equal(f.state(), undefined);
  assert.equal((await f.poll(idle)).exits, 1);
});

test('a second process of the same conversation cannot activate a shared-profile bridge', async () => {
  for (const options of [{callerPid: 8}, {callerStamp: 'reused-pid'}]) {
    const f = await fixture({late: true, ...options});
    assert.deepEqual(await f.poll(idle), {exits: 0, phase: 'unavailable'});
  }
});
test('cancellation is acknowledged during work and permanently revokes later return requests', async () => {
  const f = await fixture({late:true});
  f.request({protocol:1, token:'launch',session:'session', action:'cancel', revision:2, expiresAt:2000});
  assert.deepEqual(await f.poll({...idle, isWorking:true}), {exits:0, phase:'cancelled'});
  assert.equal(f.state().revision, 2);
  f.request({protocol:1, token:'launch',session:'session', action:'return', revision:1, expiresAt:2000});
  assert.deepEqual(await f.poll(idle), {exits:0, phase:'cancelled'});
});
test('expiry during an asynchronous prompt read cannot commit exit', async () => {
  const f = await fixture({late:true, beforePrompt({advance}) { advance(700); }});
  f.request({protocol:1,token:'launch',session:'session',action:'return',revision:1,expiresAt:2000});
  assert.equal((await f.poll(idle)).exits,0);
});
test('cancellation before commit is acknowledged, while committed exit is never falsely acknowledged', async () => {
  let reads=0;
  const f = await fixture({late:true,beforePrompt({files}) {
    if (++reads === 2) files.set('/owned/return.request',JSON.stringify({protocol:1,token:'launch',session:'session',action:'cancel',revision:2,expiresAt:2000}));
  }});
  assert.equal((await f.poll(idle)).exits,0);
  assert.deepEqual(await f.poll(idle),{exits:0,phase:'cancelled'});
  const committed = await fixture({late:true,beforeClosing(files) {
    files.set('/owned/return.request',JSON.stringify({protocol:1,token:'launch',session:'session',action:'cancel',revision:2,expiresAt:2000}));
  }});
  assert.deepEqual(await committed.poll(idle),{exits:1,phase:'closing'});
  assert.notEqual(committed.state().phase,'cancelled');
});
