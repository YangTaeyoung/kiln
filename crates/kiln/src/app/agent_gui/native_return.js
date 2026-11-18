// Runs inside the unmodified native Claude CLI through its documented mods API.
// It never sends a model prompt, edits the draft, or synthesizes terminal input.
export function register(on) {
  let snapshot = null;
  let polling = false;
  let closing = false;
  let lastState = '';
  const hasBackground = hint => /[0-9]|\bshell|\bmonitor|\bworkflow|\bcron|\bscheduled|\bloop|to manage/i.test(hint);
  on('ui.render', { component: 'PromptHint' }, ($, e, next) => {
    if (e.surface === 'terminal') {
      const props = e.props ?? {};
      snapshot = typeof props.isWorking === 'boolean' && typeof props.isDraft === 'boolean' && typeof props.hint === 'string'
        ? { ...props, at: $.clock.now() } : null;
    }
    return next(e);
  });
  on('session.start', async ($, e, next) => {
    const token = await $.env.get('KILN_NATIVE_RETURN_TOKEN');
    const session = await $.session.id();
    const commands = await $.command.list();
    if (!token || !commands.some(command => command.name === 'exit' && command.source === 'builtin')) return next(e);
    const statePath = $.plugin.root + '/bridge.state';
    const requestPath = $.plugin.root + '/return.request';
    const publish = async phase => {
      const state = JSON.stringify({ protocol: 1, token, session, phase });
      if (state !== lastState) { await $.fs.write(statePath, state); lastState = state; }
    };
    await publish('starting');
    $.clock.every(150, async () => {
      if (polling || closing) return;
      polling = true;
      try {
        $.ui.invalidate('ui.render');
        if (!snapshot || $.clock.now() - snapshot.at > 750) { await publish('starting'); return; }
        if (snapshot.isWorking) { await publish('working'); return; }
        if (snapshot.isDraft || (await $.prompt.read()).text !== '') { await publish('draft'); return; }
        // Read the native PromptHint API's task badges, never arbitrary PTY output.
        // Unknown numeric/task badges conservatively keep the native writer alive.
        if (hasBackground(snapshot.hint)) { await publish('background'); return; }
        await publish('ready');
        if (!await $.fs.exists(requestPath)) return;
        const request = JSON.parse(await $.fs.read(requestPath));
        if (request.protocol !== 1 || request.token !== token || request.session !== session) return;
        // Recheck after I/O. The native command API also queues until its own idle
        // boundary, so a late model turn is allowed to finish without interruption.
        if (!snapshot || $.clock.now() - snapshot.at > 750 || snapshot.isWorking || snapshot.isDraft || hasBackground(snapshot.hint) || (await $.prompt.read()).text !== '') return;
        closing = true;
        await publish('closing');
        try { await $.command.run({ command: 'exit' }); }
        catch { closing = false; await publish('unavailable'); }
      } catch {
        // Partial writes, disabled extensions and storage errors never prove idle.
      } finally { polling = false; }
    });
    return next(e);
  });
}
