use anyhow::{anyhow, Error, Result};
use std::sync::Mutex;

static PIN_TRANSACTION: Mutex<()> = Mutex::new(());

pub(crate) trait PinTarget {
    fn current_pinned(&self) -> Result<bool>;
    fn set_native_pinned(&mut self, pinned: bool) -> Result<()>;
    fn set_durable_pinned(&mut self, pinned: bool) -> Result<()>;
    fn sync_pinned_registry(&mut self) -> Result<()>;
    fn focus(&mut self) -> Result<()>;
}

pub(crate) struct PinWorkflow;

impl PinWorkflow {
    pub(crate) fn perform(target: &mut impl PinTarget, pinned: bool) -> Result<()> {
        {
            let _transaction = PIN_TRANSACTION
                .lock()
                .map_err(|_| anyhow!("Pin transaction lock poisoned"))?;
            let previous = target.current_pinned()?;
            target.set_native_pinned(pinned)?;

            if let Err(error) = target.set_durable_pinned(pinned) {
                let rollback = target
                    .set_native_pinned(previous)
                    .err()
                    .map(|error| ("native", error));
                return Err(with_compensation_failures(error, rollback.into_iter()));
            }

            if let Err(error) = target.sync_pinned_registry() {
                let durable_rollback = target
                    .set_durable_pinned(previous)
                    .err()
                    .map(|error| ("durable", error));
                let native_rollback = target
                    .set_native_pinned(previous)
                    .err()
                    .map(|error| ("native", error));
                return Err(with_compensation_failures(
                    error,
                    durable_rollback.into_iter().chain(native_rollback),
                ));
            }
        }

        target.focus()
    }
}

fn with_compensation_failures(
    primary: Error,
    failures: impl Iterator<Item = (&'static str, Error)>,
) -> Error {
    let failures: Vec<_> = failures
        .map(|(step, error)| format!("{step} rollback: {error:#}"))
        .collect();
    if failures.is_empty() {
        primary
    } else {
        anyhow!("{primary:#}; compensation failed: {}", failures.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{bail, Result};
    use std::collections::HashSet;
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct MemoryPinTarget {
        native: bool,
        durable: bool,
        registry: bool,
        focused: bool,
        events: Vec<String>,
        failures: HashSet<&'static str>,
    }

    impl MemoryPinTarget {
        fn with_failure(step: &'static str) -> Self {
            Self {
                failures: HashSet::from([step]),
                ..Self::default()
            }
        }

        fn fail(&self, step: &'static str) -> Result<()> {
            if self.failures.contains(step) {
                bail!("{step} failed");
            }
            Ok(())
        }
    }

    impl PinTarget for MemoryPinTarget {
        fn current_pinned(&self) -> Result<bool> {
            Ok(self.durable)
        }

        fn set_native_pinned(&mut self, pinned: bool) -> Result<()> {
            let step = if pinned { "native" } else { "native rollback" };
            self.events.push(step.into());
            self.fail(step)?;
            self.native = pinned;
            Ok(())
        }

        fn set_durable_pinned(&mut self, pinned: bool) -> Result<()> {
            let step = if pinned {
                "durable"
            } else {
                "durable rollback"
            };
            self.events.push(step.into());
            self.fail(step)?;
            self.durable = pinned;
            Ok(())
        }

        fn sync_pinned_registry(&mut self) -> Result<()> {
            let step = if self.durable {
                "registry"
            } else {
                "registry rollback"
            };
            self.events.push(step.into());
            self.fail(step)?;
            self.registry = self.durable;
            Ok(())
        }

        fn focus(&mut self) -> Result<()> {
            self.events.push("focus".into());
            self.fail("focus")?;
            self.focused = true;
            Ok(())
        }
    }

    #[test]
    fn pin_success_aligns_all_representations_and_focuses() {
        let mut target = MemoryPinTarget::default();

        PinWorkflow::perform(&mut target, true).unwrap();

        assert!(target.native);
        assert!(target.durable);
        assert!(target.registry);
        assert!(target.focused);
        assert_eq!(target.events, ["native", "durable", "registry", "focus"]);
    }

    #[test]
    fn unpin_success_aligns_all_representations() {
        let mut target = MemoryPinTarget {
            native: true,
            durable: true,
            registry: true,
            ..MemoryPinTarget::default()
        };

        PinWorkflow::perform(&mut target, false).unwrap();

        assert!(!target.native);
        assert!(!target.durable);
        assert!(!target.registry);
        assert!(target.focused);
    }

    #[test]
    fn native_failure_stops_before_durable_and_registry_changes() {
        let mut target = MemoryPinTarget::with_failure("native");

        assert_eq!(
            PinWorkflow::perform(&mut target, true)
                .unwrap_err()
                .to_string(),
            "native failed"
        );
        assert!(!target.native);
        assert!(!target.durable);
        assert!(!target.registry);
        assert_eq!(target.events, ["native"]);
    }

    #[test]
    fn durable_failure_restores_native_state() {
        let mut target = MemoryPinTarget::with_failure("durable");

        assert_eq!(
            PinWorkflow::perform(&mut target, true)
                .unwrap_err()
                .to_string(),
            "durable failed"
        );
        assert!(!target.native);
        assert!(!target.durable);
        assert!(!target.registry);
        assert_eq!(target.events, ["native", "durable", "native rollback"]);
    }

    #[test]
    fn durable_failure_reports_a_failed_native_rollback() {
        let mut target = MemoryPinTarget {
            failures: HashSet::from(["durable", "native rollback"]),
            ..MemoryPinTarget::default()
        };

        let error = PinWorkflow::perform(&mut target, true)
            .unwrap_err()
            .to_string();

        assert!(error.contains("durable failed"));
        assert!(error.contains("native rollback failed"));
        assert!(target.native);
        assert!(!target.durable);
        assert!(!target.registry);
        assert_eq!(target.events, ["native", "durable", "native rollback"]);
    }

    #[test]
    fn registry_failure_restores_durable_and_native_state() {
        let mut target = MemoryPinTarget::with_failure("registry");

        assert_eq!(
            PinWorkflow::perform(&mut target, true)
                .unwrap_err()
                .to_string(),
            "registry failed"
        );
        assert!(!target.native);
        assert!(!target.durable);
        assert!(!target.registry);
        assert_eq!(
            target.events,
            [
                "native",
                "durable",
                "registry",
                "durable rollback",
                "native rollback"
            ]
        );
    }

    #[test]
    fn compensation_failures_are_reported_together_and_all_are_attempted() {
        let mut target = MemoryPinTarget {
            failures: HashSet::from(["registry", "durable rollback", "native rollback"]),
            ..MemoryPinTarget::default()
        };

        let error = PinWorkflow::perform(&mut target, true)
            .unwrap_err()
            .to_string();

        assert!(error.contains("registry failed"));
        assert!(error.contains("durable rollback failed"));
        assert!(error.contains("native rollback failed"));
        assert!(target.native);
        assert!(target.durable);
        assert!(!target.registry);
        assert!(!target.focused);
        assert_eq!(
            target.events,
            [
                "native",
                "durable",
                "registry",
                "durable rollback",
                "native rollback"
            ]
        );
    }

    struct BlockingPinTarget {
        started: Option<mpsc::Sender<()>>,
        entered: Option<mpsc::Sender<()>>,
        release: Option<Arc<(Mutex<bool>, Condvar)>>,
        pinned: bool,
    }

    impl PinTarget for BlockingPinTarget {
        fn current_pinned(&self) -> Result<bool> {
            if let Some(entered) = &self.entered {
                entered.send(()).unwrap();
            }
            Ok(self.pinned)
        }

        fn set_native_pinned(&mut self, pinned: bool) -> Result<()> {
            if let Some(started) = self.started.take() {
                started.send(()).unwrap();
            }
            if let Some(release) = &self.release {
                let (released, ready) = &**release;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
            }
            self.pinned = pinned;
            Ok(())
        }

        fn set_durable_pinned(&mut self, pinned: bool) -> Result<()> {
            self.pinned = pinned;
            Ok(())
        }

        fn sync_pinned_registry(&mut self) -> Result<()> {
            Ok(())
        }

        fn focus(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pin_transactions_are_serialized_across_targets() {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let first_release = Arc::clone(&release);
        let first = std::thread::spawn(move || {
            PinWorkflow::perform(
                &mut BlockingPinTarget {
                    started: Some(first_started_tx),
                    entered: None,
                    release: Some(first_release),
                    pinned: false,
                },
                true,
            )
        });
        first_started_rx.recv().unwrap();

        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            PinWorkflow::perform(
                &mut BlockingPinTarget {
                    started: None,
                    entered: Some(second_entered_tx),
                    release: None,
                    pinned: false,
                },
                true,
            )
        });
        let second_was_blocked = matches!(
            second_entered_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        let (released, ready) = &*release;
        *released.lock().unwrap() = true;
        ready.notify_one();

        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(second_was_blocked);
    }
}
