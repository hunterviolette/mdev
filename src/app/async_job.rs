use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct AsyncLatestJob<R> {
    generation: u64,
    pending_generation: Option<u64>,
    rx: Option<Receiver<(u64, Result<R, String>)>>,
}

impl<R> Default for AsyncLatestJob<R> {
    fn default() -> Self {
        Self {
            generation: 0,
            pending_generation: None,
            rx: None,
        }
    }
}

impl<R: Send + 'static> AsyncLatestJob<R> {
    pub fn start_latest<F>(&mut self, work: F)
    where
        F: FnOnce() -> Result<R, String> + Send + 'static,
    {
        let generation = self.generation.wrapping_add(1);
        self.generation = generation;
        self.pending_generation = Some(generation);

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((generation, work()));
        });
        self.rx = Some(rx);
    }

    pub fn poll(&mut self) -> Option<(u64, Result<R, String>)> {
        let recv = match self.rx.as_ref() {
            Some(rx) => rx.try_recv(),
            None => return None,
        };

        match recv {
            Ok((generation, result)) => {
                self.rx = None;
                if self.pending_generation == Some(generation) {
                    self.pending_generation = None;
                    Some((generation, result))
                } else {
                    None
                }
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.rx = None;
                self.pending_generation = None;
                Some((
                    self.generation,
                    Err("Background job channel disconnected.".to_string()),
                ))
            }
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending_generation.is_some()
    }

    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending_generation = None;
        self.rx = None;
    }
}

pub struct AsyncQueuedJob<I, R> {
    queued: VecDeque<I>,
    current: AsyncLatestJob<R>,
    total: usize,
    completed: usize,
}

impl<I, R> Default for AsyncQueuedJob<I, R> {
    fn default() -> Self {
        Self {
            queued: VecDeque::new(),
            current: AsyncLatestJob::default(),
            total: 0,
            completed: 0,
        }
    }
}

impl<I: Send + 'static, R: Send + 'static> AsyncQueuedJob<I, R> {
    pub fn start_queue(&mut self, items: Vec<I>) {
        self.current.cancel();
        self.queued = items.into();
        self.total = self.queued.len();
        self.completed = 0;
    }

    pub fn clear(&mut self) {
        self.current.cancel();
        self.queued.clear();
        self.total = 0;
        self.completed = 0;
    }

    pub fn is_pending(&self) -> bool {
        self.current.is_pending() || !self.queued.is_empty()
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.completed, self.total)
    }

    pub fn poll_with<F>(&mut self, run: F) -> Option<(usize, usize, Result<R, String>)>
    where
        F: Fn(I) -> Result<R, String> + Send + Sync + Clone + 'static,
    {
        if !self.current.is_pending() {
            if let Some(item) = self.queued.pop_front() {
                let run_next = run.clone();
                self.current.start_latest(move || run_next(item));
            }
        }

        let Some((_, result)) = self.current.poll() else {
            return None;
        };

        self.completed = self.completed.saturating_add(1);
        Some((self.completed, self.total, result))
    }
}
