use std::sync::mpsc::{self, Receiver, TryRecvError};

/// 백그라운드 스레드에서 실행되는 단발성 작업. 완료되면 egui 컨텍스트에 리페인트를 요청한다.
pub struct Task<T> {
    rx: Option<Receiver<T>>,
    result: Option<T>,
}

impl<T: Send + 'static> Task<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let v = f();
            let _ = tx.send(v);
            ctx.request_repaint();
        });
        Self { rx: Some(rx), result: None }
    }

    /// 결과 없이 즉시 완료된 작업.
    pub fn ready(v: T) -> Self {
        Self { rx: None, result: Some(v) }
    }

    fn pump(&mut self) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(v) => {
                    self.result = Some(v);
                    self.rx = None;
                }
                Err(TryRecvError::Disconnected) => self.rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    pub fn is_pending(&mut self) -> bool {
        self.pump();
        self.rx.is_some()
    }

    pub fn poll(&mut self) -> Option<&T> {
        self.pump();
        self.result.as_ref()
    }

    pub fn take(&mut self) -> Option<T> {
        self.pump();
        self.result.take()
    }

    /// 완료까지 블로킹한다. 테스트용.
    pub fn wait(mut self) -> Option<T> {
        if let Some(rx) = self.rx.take() {
            return rx.recv().ok();
        }
        self.result.take()
    }
}
