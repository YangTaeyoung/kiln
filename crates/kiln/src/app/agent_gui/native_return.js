// Runs inside the unmodified native Claude CLI through its documented mods API.
// It never sends a model prompt, edits the draft, or synthesizes terminal input.
function hasBackground(hint) {
  return /[0-9]|\bshell|\bmonitor|\bworkflow|\bcron|\bscheduled|\bloop|to manage/i.test(hint);
}
// Claude validates API-bearing helpers at module scope, including on reload.
async function initialize($, state, terminalRender) {
  if (state.initialized || state.initializing) return;
  state.initializing = true;
  try {
    const session = await $.session.id();
    let token;
    const configPath = $.plugin.root + '/bridge.config';
    if (await $.fs.exists(configPath)) {
      // The resumed JSON backend may start before profile cleanup finishes.
      // A late bridge belongs only to a real terminal frontend.
      if (!terminalRender) return;
      const config = JSON.parse(await $.fs.read(configPath));
      // A late-loaded plugin is visible in a shared profile. Only its explicitly
      // named conversation may publish state or install an idle-exit timer.
      if (config.protocol !== 1 || config.session !== session || typeof config.token !== 'string' || !config.token) {
        state.initialized = true;
        return;
      }
      // A shared profile and even a shared conversation UUID do not prove that
      // this is the selected terminal. Ask an OS helper launched by this process.
      const result = await $.process.run([config.helper, 'agent-native-identity'], { timeoutMs: 2000 });
      if (result.exitCode !== 0 || result.isStdoutTruncated) return;
      const caller = JSON.parse(result.stdout);
      if (caller.protocol !== 1 || caller.pid !== config.pid || caller.stamp !== config.stamp) {
        state.initialized = true;
        return;
      }
      token = config.token;
    } else {
      token = await $.env.get('KILN_NATIVE_RETURN_TOKEN');
    }
    const commands = await $.command.list();
    if (!token || !commands.some(command => command.name === 'exit' && command.source === 'builtin')) return;
    const statePath = $.plugin.root + '/bridge.state';
    const requestPath = $.plugin.root + '/return.request';
    const publish = async phase => {
      const payload = JSON.stringify({ protocol: 1, token, session, phase, revision: state.cancelled ? 2 : 1 });
      if (payload !== state.lastState) { await $.fs.write(statePath, payload); state.lastState = payload; }
    };
    await publish('starting');
    state.initialized = true;
    $.clock.every(150, async () => {
      if (state.polling || state.closing) return;
      state.polling = true;
      try {
        const validRequest = async () => {
          if (!await $.fs.exists(requestPath)) return null;
          const request = JSON.parse(await $.fs.read(requestPath));
          if (request.protocol !== 1 || request.token !== token || request.session !== session) return null;
          if (request.expiresAt !== undefined && (typeof request.expiresAt !== 'number' || !Number.isFinite(request.expiresAt) || request.expiresAt <= $.clock.now())) return null;
          return request;
        };
        const control = await validRequest();
        // A cancellation is acknowledged before idle checks, including while a
        // model, draft or shell is active. Acknowledgment permanently revokes exit.
        if (state.cancelled || (control?.action === 'cancel' && control.revision === 2)) {
          state.cancelled = true;
          await publish('cancelled');
          return;
        }
        $.ui.invalidate('ui.render');
        if (!state.snapshot || $.clock.now() - state.snapshot.at > 750) { await publish('starting'); return; }
        if (state.snapshot.isWorking) { await publish('working'); return; }
        if (state.snapshot.isDraft || (await $.prompt.read()).text !== '') { await publish('draft'); return; }
        if (hasBackground(state.snapshot.hint)) { await publish('background'); return; }
        await publish('ready');
        const request = await validRequest();
        if (!request || request.action === 'cancel') return;
        if (!state.snapshot || $.clock.now() - state.snapshot.at > 750 || state.snapshot.isWorking || state.snapshot.isDraft || hasBackground(state.snapshot.hint) || (await $.prompt.read()).text !== '') return;
        const commit = await validRequest();
        if (!commit || commit.action === 'cancel') return;
        // No await between this decision and closing: cancellation is either
        // acknowledged next tick, or the GUI retains takeover after committed exit.
        state.closing = true;
        await publish('closing');
        try { await $.command.run({ command: 'exit' }); }
        catch { state.closing = false; await publish('unavailable'); }
      } catch {
        // Partial writes, disabled extensions and storage errors never prove idle.
      } finally { state.polling = false; }
    });
  } catch (error) {
    await $.fs.write($.plugin.root + '/bridge.error', JSON.stringify({message:String(error)}));
  } finally { state.initializing = false; }
}
export function register(on) {
  const state = { snapshot: null, polling: false, closing: false, initialized: false, initializing: false, lastState: '' };
  on('ui.render', { component: 'PromptHint' }, async ($, e, next) => {
    if (e.surface === 'terminal') {
      const props = e.props ?? {};
      state.snapshot = typeof props.isWorking === 'boolean' && typeof props.isDraft === 'boolean' && typeof props.hint === 'string'
        ? { isWorking: props.isWorking, isDraft: props.isDraft, hint: props.hint, at: $.clock.now() } : null;
      // ui.render can be aborted by a newer frame. Process authentication must
      // finish on a module-owned timer rather than a superseded render request.
      if (!state.initialized && !state.scheduled) {
        state.scheduled = true;
        $.clock.every(150, async () => { if (!state.initialized) await initialize($, state, true); });
      }
    }
    return next(e);
  });
  on('session.start', async ($, e, next) => {
    await initialize($, state, false);
    return next(e);
  });
}
