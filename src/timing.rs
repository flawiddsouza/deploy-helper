use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::Instant;

#[derive(Default)]
struct Counts {
    succeeded: usize,
    failed: usize,
    skipped: usize,
    recovery: usize,
}

pub struct RunTiming {
    started: Instant,
    counts: Rc<RefCell<Counts>>,
}

impl RunTiming {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            counts: Rc::new(RefCell::new(Counts::default())),
        }
    }

    pub fn skip(&self) {
        self.counts.borrow_mut().skipped += 1;
    }

    pub fn task(&self, name: &str, no_log: bool, recovery: bool) -> TaskTiming {
        TaskTiming {
            started: Instant::now(),
            name: if no_log {
                "[no_log]".into()
            } else {
                name.into()
            },
            counts: Rc::clone(&self.counts),
            succeeded: false,
            skipped: false,
            recovery,
        }
    }
}

impl Drop for RunTiming {
    fn drop(&mut self) {
        let counts = self.counts.borrow();
        // A closed output pipe must not panic while another error is unwinding.
        let _ = writeln!(
            io::stdout().lock(),
            "Run summary: {} succeeded, {} failed, {} skipped, {} recovery tasks; {:.3}s total",
            counts.succeeded,
            counts.failed,
            counts.skipped,
            counts.recovery,
            self.started.elapsed().as_secs_f64()
        );
    }
}

pub struct TaskTiming {
    started: Instant,
    name: String,
    counts: Rc<RefCell<Counts>>,
    pub succeeded: bool,
    skipped: bool,
    recovery: bool,
}

impl TaskTiming {
    pub fn skip(mut self) {
        self.skipped = true;
    }
}

impl Drop for TaskTiming {
    fn drop(&mut self) {
        let mut counts = self.counts.borrow_mut();
        if self.recovery && !self.skipped {
            counts.recovery += 1;
        }
        if self.skipped {
            counts.skipped += 1;
        } else if self.succeeded {
            counts.succeeded += 1;
        } else {
            counts.failed += 1;
        }
        let _ = writeln!(
            io::stdout().lock(),
            "Task elapsed: {:.3}s [{}] {}",
            self.started.elapsed().as_secs_f64(),
            if self.skipped {
                "skipped"
            } else if self.succeeded {
                "ok"
            } else {
                "failed"
            },
            self.name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_outcomes_and_redacts_secret_names() {
        let run = RunTiming::new();
        run.skip();
        run.task("Skipped cleanup", false, true).skip();
        {
            let mut task = run.task("success", false, false);
            task.succeeded = true;
        }
        {
            let task = run.task("secret", true, true);
            assert_eq!(task.name, "[no_log]");
        }
        let counts = run.counts.borrow();
        assert_eq!(
            (
                counts.succeeded,
                counts.failed,
                counts.skipped,
                counts.recovery
            ),
            (1, 1, 2, 1)
        );
    }
}
