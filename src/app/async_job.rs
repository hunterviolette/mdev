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

#[derive(Clone, Debug)]
pub enum AsyncParallelEvent<I, R> {
    Started {
        item: I,
        completed: usize,
        total: usize,
    },
    Finished {
        item: I,
        completed: usize,
        total: usize,
        result: Result<R, String>,
    },
}

pub struct AsyncParallelJob<I, R> {
    pending: VecDeque<I>,
    running: Vec<(I, Receiver<Result<R, String>>)>,
    total: usize,
    completed: usize,
    max_concurrency: usize,
}

impl<I, R> Default for AsyncParallelJob<I, R> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            running: Vec::new(),
            total: 0,
            completed: 0,
            max_concurrency: 1,
        }
    }
}

impl<I: Clone + Send + 'static, R: Send + 'static> AsyncParallelJob<I, R> {
    pub fn start_batch(&mut self, items: Vec<I>, max_concurrency: usize) {
        self.clear();
        self.pending = items.into();
        self.total = self.pending.len();
        self.completed = 0;
        self.max_concurrency = max_concurrency.max(1);
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.running.clear();
        self.total = 0;
        self.completed = 0;
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty() || !self.running.is_empty()
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.completed, self.total)
    }

    pub fn poll_events_with<F>(&mut self, run: F) -> Vec<AsyncParallelEvent<I, R>>
    where
        F: Fn(I) -> Result<R, String> + Send + Sync + Clone + 'static,
    {
        let mut events = Vec::new();

        while self.running.len() < self.max_concurrency {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            let item_for_worker = item.clone();
            let item_for_event = item.clone();
            let run_next = run.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(run_next(item_for_worker));
            });
            self.running.push((item.clone(), rx));
            events.push(AsyncParallelEvent::Started {
                item: item_for_event,
                completed: self.completed,
                total: self.total,
            });
        }

        let mut idx = 0;
        while idx < self.running.len() {
            let recv = {
                let (_, rx) = &self.running[idx];
                rx.try_recv()
            };
            match recv {
                Ok(result) => {
                    let (item, _) = self.running.swap_remove(idx);
                    self.completed = self.completed.saturating_add(1);
                    events.push(AsyncParallelEvent::Finished {
                        item,
                        completed: self.completed,
                        total: self.total,
                        result,
                    });
                }
                Err(TryRecvError::Empty) => {
                    idx += 1;
                }
                Err(TryRecvError::Disconnected) => {
                    let (item, _) = self.running.swap_remove(idx);
                    self.completed = self.completed.saturating_add(1);
                    events.push(AsyncParallelEvent::Finished {
                        item,
                        completed: self.completed,
                        total: self.total,
                        result: Err("Background job channel disconnected.".to_string()),
                    });
                }
            }
        }

        events
    }
}
