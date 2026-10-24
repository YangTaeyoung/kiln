//! Output can be batched without delaying keyboard echo or important events.
use super::*;
use kiln_daemon::client::NotifyKind;
use parking_lot::Mutex;

const OUTPUT_INTERVAL: Duration = Duration::from_millis(33);
const INPUT_PRIORITY: Duration = Duration::from_millis(250);
pub(super) struct Repaint { state: Mutex<State> }
struct State { frame: Instant, input: Option<Instant>, output_queued: bool }
impl Default for Repaint {
    fn default() -> Self { Self { state: Mutex::new(State { frame: Instant::now(), input: None, output_queued: false }) } }
}
impl Repaint {
    pub fn frame(&self) { let mut state = self.state.lock(); state.frame = Instant::now(); state.output_queued = false; }
    pub fn input(&self) { self.state.lock().input = Some(Instant::now()); }
    fn delay(&self, kind: NotifyKind) -> Duration {
        match kind {
            NotifyKind::Event => Duration::ZERO,
            NotifyKind::Status => STATUS_REPAINT_DELAY,
            NotifyKind::Frame => {
                let state = self.state.lock();
                if state.input.is_some_and(|at| at.elapsed() < INPUT_PRIORITY) { Duration::ZERO }
                else { (state.frame + OUTPUT_INTERVAL).saturating_duration_since(Instant::now()) }
            }
        }
    }
    pub fn notify(&self, kind: NotifyKind, ctx: &egui::Context) {
        if kind == NotifyKind::Frame {
            let mut state = self.state.lock();
            if !state.input.is_some_and(|at| at.elapsed() < INPUT_PRIORITY) {
                if state.output_queued { return; }
                state.output_queued = true;
            }
        }
        let delay = self.delay(kind);
        if delay.is_zero() { ctx.request_repaint(); }
        else {
            // egui subtracts predicted_dt internally; preserve the output deadline.
            let predicted = ctx.input(|input| Duration::try_from_secs_f32(input.predicted_dt).unwrap_or_default());
            ctx.request_repaint_after(delay + predicted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_output_waits_but_input_echo_and_events_are_immediate() {
        let repaint = Repaint::default();
        assert!(repaint.delay(NotifyKind::Frame) > Duration::from_millis(20));
        assert_eq!(repaint.delay(NotifyKind::Event), Duration::ZERO);
        assert_eq!(repaint.delay(NotifyKind::Status), STATUS_REPAINT_DELAY);
        repaint.input();
        assert_eq!(repaint.delay(NotifyKind::Frame), Duration::ZERO);
    }
    #[test]
    fn real_context_coalesces_output_and_input_advances_the_deadline() {
        let ctx = egui::Context::default();
        for _ in 0..3 { ctx.begin_pass(Default::default()); let mut output = ctx.end_pass(); output.textures_delta.clear(); }
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = calls.clone();
        ctx.set_request_repaint_callback(move |info| output.lock().unwrap().push(info.delay));
        let repaint = Repaint::default();
        for _ in 0..100 { repaint.notify(NotifyKind::Frame, &ctx); }
        let delayed = calls.lock().unwrap().clone();
        assert_eq!(delayed.len(), 1, "one queued redraw is enough for a burst of output");
        assert!(delayed.iter().all(|d| *d > Duration::from_millis(20)));
        repaint.input();
        repaint.notify(NotifyKind::Frame, &ctx);
        assert_eq!(*calls.lock().unwrap().last().unwrap(), Duration::ZERO);
    }
}
