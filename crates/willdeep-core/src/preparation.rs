//! Opt-in preparation timings. Only static stage names and durations reach stderr.

use std::io::Write;
use std::time::Instant;

/// Enable with `WILLDEEP_TRACE_PREPARATION=1`; never records task or configuration data.
pub struct PreparationTrace {
    component: &'static str,
    clock: Option<(Instant, Instant)>,
}

impl PreparationTrace {
    pub fn new(component: &'static str) -> Self {
        let enabled =
            std::env::var_os("WILLDEEP_TRACE_PREPARATION").is_some_and(|value| value == "1");
        Self {
            component,
            clock: enabled.then(|| {
                let now = Instant::now();
                (now, now)
            }),
        }
    }

    pub fn mark(&mut self, stage: &'static str) {
        if self.clock.is_some() {
            self.mark_to(stage, &mut std::io::stderr().lock());
        }
    }

    fn mark_to(&mut self, stage: &'static str, output: &mut impl Write) {
        if let Some((started, previous)) = &mut self.clock {
            let now = Instant::now();
            let record = serde_json::json!({
                "schema": "willdeep.preparation.v1",
                "component": self.component,
                "stage": stage,
                "elapsed_ms": now.duration_since(*previous).as_secs_f64() * 1000.0,
                "total_ms": now.duration_since(*started).as_secs_f64() * 1000.0,
            });
            if writeln!(output, "{record}").is_err() {
                // Diagnostics are best effort. Disable after an I/O failure so
                // Drop cannot panic or retry a broken stderr during unwinding.
                self.clock = None;
                return;
            }
            *previous = now;
        }
    }
}

impl Drop for PreparationTrace {
    fn drop(&mut self) {
        self.mark("finished");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenOutput(usize);

    impl Write for BrokenOutput {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_diagnostic_output_disables_tracing_without_panicking() {
        let now = Instant::now();
        let mut trace = PreparationTrace {
            component: "test",
            clock: Some((now, now)),
        };
        let mut output = BrokenOutput(0);
        trace.mark_to("first", &mut output);
        trace.mark_to("second", &mut output);
        assert_eq!(output.0, 1);
        assert!(trace.clock.is_none());
        drop(trace);
    }
}
