use std::future::Future;
use std::time::Duration;

/// How an integration confirms its dependency answers before the application serves.
///
/// A database module bounds each attempt with its driver's own connect or acquire timeout, then
/// retries on this schedule, so an unreachable server fails
/// [`ToniFactory::create`](crate::ToniFactory::create) with a
/// [`StartupError::HookFailed`](crate::StartupError::HookFailed) naming the module, rather than
/// surfacing as errors on the first request that needs it.
///
/// ```ignore
/// SeaOrmModule::for_root(url)                            // checked, with the defaults
/// SeaOrmModule::for_root(url).without_startup_check()    // start regardless
/// RedisModule::for_root(url)
///     .with_startup_check(StartupCheck::default().attempts(5))
/// ```
///
/// The policy lives here rather than in each integration because the drivers disagree about
/// retrying: one counts attempts with its own exponential backoff, another takes a total timeout
/// and no attempt count, two do not retry at all. Their internal retry is switched off and this
/// schedule is used instead, so every integration gives up at the same point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupCheck {
    attempts: u32,
    delay: Duration,
    timeout: Duration,
}

impl Default for StartupCheck {
    /// Three attempts, two seconds apart, five seconds each: enough for a database container that
    /// starts a little after the application, and bounded so a dead dependency cannot hold startup
    /// past a readiness deadline.
    fn default() -> Self {
        Self {
            attempts: 3,
            delay: Duration::from_secs(2),
            timeout: Duration::from_secs(5),
        }
    }
}

impl StartupCheck {
    /// Total attempts, including the first. Values below 1 are treated as 1.
    pub fn attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts.max(1);
        self
    }

    /// How long to wait between attempts.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// How long one attempt may take. An integration hands this to its driver, which is what
    /// bounds the probe.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Attempts this check will make.
    pub fn attempt_count(&self) -> u32 {
        self.attempts
    }

    /// Gap between attempts.
    pub fn retry_delay(&self) -> Duration {
        self.delay
    }

    /// Bound for a single attempt, for the driver to enforce.
    pub fn attempt_timeout(&self) -> Duration {
        self.timeout
    }

    /// The longest this check can take before it gives up.
    pub fn worst_case(&self) -> Duration {
        self.timeout * self.attempts + self.delay * self.attempts.saturating_sub(1)
    }

    /// Runs `probe` until it answers or the attempts are spent, returning the last failure.
    ///
    /// `probe` is called once per attempt, so it must be able to run more than once. Bounding an
    /// attempt is the caller's job — it configures the driver with [`attempt_timeout`] before
    /// probing, which is why nothing here races a timer.
    ///
    /// `sleep` supplies only the gap between attempts, so a check that answers first time never
    /// constructs one. Core takes no dependency on it: an integration passes
    /// `futures_timer::Delay::new`, or a caller on tokio could pass `tokio::time::sleep`.
    ///
    /// [`attempt_timeout`]: StartupCheck::attempt_timeout
    pub async fn run<P, PFut, S, SFut>(&self, probe: P, sleep: S) -> Result<(), String>
    where
        P: Fn() -> PFut,
        PFut: Future<Output = Result<(), String>>,
        S: Fn(Duration) -> SFut,
        SFut: Future<Output = ()>,
    {
        let mut last = String::from("no attempt was made");

        for attempt in 1..=self.attempts {
            match probe().await {
                Ok(()) => return Ok(()),
                Err(failure) => last = failure,
            }

            if attempt < self.attempts {
                tracing::debug!(
                    attempt,
                    of = self.attempts,
                    error = %last,
                    "startup check failed, retrying"
                );
                sleep(self.delay).await;
            }
        }

        Err(if self.attempts == 1 {
            last
        } else {
            format!("{last} (after {} attempts)", self.attempts)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A probe that answers is run once, and never reaches for the timer.
    #[test]
    fn a_probe_that_answers_never_sleeps() {
        let probes = Cell::new(0);
        let sleeps = Cell::new(0);

        let result = futures_executor::block_on(StartupCheck::default().run(
            || {
                probes.set(probes.get() + 1);
                async { Ok(()) }
            },
            |_| {
                sleeps.set(sleeps.get() + 1);
                async {}
            },
        ));

        assert!(result.is_ok());
        assert_eq!(probes.get(), 1);
        assert_eq!(sleeps.get(), 0, "the happy path must not construct a timer");
    }

    #[test]
    fn a_probe_that_recovers_is_retried_until_it_answers() {
        let probes = Cell::new(0);

        let result = futures_executor::block_on(StartupCheck::default().run(
            || {
                probes.set(probes.get() + 1);
                let attempt = probes.get();
                async move {
                    if attempt < 3 {
                        Err("refused".to_string())
                    } else {
                        Ok(())
                    }
                }
            },
            |_| async {},
        ));

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(probes.get(), 3);
    }

    #[test]
    fn the_last_failure_is_reported_once_the_attempts_are_spent() {
        let error = futures_executor::block_on(StartupCheck::default().attempts(2).run(
            || async { Err("connection refused".to_string()) },
            |_| async {},
        ))
        .expect_err("a probe that never answers must fail");

        assert!(error.contains("connection refused"), "{error}");
        assert!(error.contains("2 attempts"), "{error}");
    }

    /// The gaps are between attempts, not after the last one.
    #[test]
    fn the_delay_is_skipped_after_the_final_attempt() {
        let sleeps = Cell::new(0);

        let _ = futures_executor::block_on(StartupCheck::default().attempts(3).run(
            || async { Err("refused".to_string()) },
            |_| {
                sleeps.set(sleeps.get() + 1);
                async {}
            },
        ));

        assert_eq!(sleeps.get(), 2);
    }

    #[test]
    fn attempts_is_never_zero() {
        assert_eq!(StartupCheck::default().attempts(0).attempt_count(), 1);
    }

    #[test]
    fn worst_case_counts_the_gaps_between_attempts_not_after_the_last() {
        let check = StartupCheck::default()
            .attempts(3)
            .timeout(Duration::from_secs(5))
            .delay(Duration::from_secs(2));
        assert_eq!(check.worst_case(), Duration::from_secs(19));
    }
}
