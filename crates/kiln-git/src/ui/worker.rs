//! 순차 실행 작업 큐. 저장소를 바꾸는 명령을 한 스레드에서 차례로 실행한다.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::cmd::GitResult;

type JobFn = Box<dyn FnOnce() -> GitResult<String> + Send>;

/// 완료된 작업 결과.
pub(crate) struct JobDone<K> {
    pub kind: K,
    pub label: String,
    pub result: GitResult<String>,
}

/// 백그라운드 작업 큐.
pub(crate) struct Worker<K: Send + 'static> {
    tx: Sender<(K, String, JobFn)>,
    rx: Receiver<JobDone<K>>,
    pending: Arc<AtomicUsize>,
    running_label: Arc<parking_lot::Mutex<Option<String>>>,
}

impl<K: Send + 'static> Worker<K> {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, job_rx) = channel::<(K, String, JobFn)>();
        let (done_tx, rx) = channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let running_label = Arc::new(parking_lot::Mutex::new(None));
        let ctx = ctx.clone();
        let p = pending.clone();
        let rl = running_label.clone();
        std::thread::Builder::new()
            .name("kiln-git-worker".into())
            .spawn(move || {
                while let Ok((kind, label, f)) = job_rx.recv() {
                    *rl.lock() = Some(label.clone());
                    ctx.request_repaint();
                    let result = f();
                    *rl.lock() = None;
                    let _ = done_tx.send(JobDone { kind, label, result });
                    p.fetch_sub(1, Ordering::SeqCst);
                    ctx.request_repaint();
                }
            })
            .expect("spawn git worker");
        Self { tx, rx, pending, running_label }
    }

    pub fn submit(&self, kind: K, label: impl Into<String>, f: impl FnOnce() -> GitResult<String> + Send + 'static) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.tx.send((kind, label.into(), Box::new(f))).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// 실행 중인 작업 이름.
    pub fn running(&self) -> Option<String> {
        self.running_label.lock().clone()
    }

    pub fn drain(&self) -> Vec<JobDone<K>> {
        self.rx.try_iter().collect()
    }
}
