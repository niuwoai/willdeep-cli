//! Complete, bounded binary capture for fixed programs consuming host snapshots.
//! Unlike run_capture, overflow is an error: no tail or omission marker is JSON.
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::sandbox::SandboxSpec;

/// The caller supplies a canonical executable and fixed argv, never shell text.
/// A fresh cwd also serves as HOME; ambient parser/preload/startup variables are
/// not inherited. Existing shell-command capture semantics remain unchanged.
pub(crate) async fn run_capture_stdin(
    executable: &Path,
    arguments: &[String],
    directory: &Path,
    sandbox: &SandboxSpec,
    input: &[u8],
    timeout: Duration,
    output_limit: usize,
) -> io::Result<std::process::Output> {
    if !executable.is_absolute() || !directory.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "absolute executable and cwd required",
        ));
    }
    let mut process = fixed_program(executable, arguments, sandbox)?;
    process
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        process.env("SystemRoot", system_root);
    }
    #[cfg(unix)]
    process.as_std_mut().process_group(0);
    #[cfg(windows)]
    let mut group = super::windows_job::Job::new(&mut process)?;
    let mut child = process.spawn()?;
    #[cfg(unix)]
    let mut group = super::ProcessGroup(child.id());
    #[cfg(windows)]
    group.attach_and_resume(&child)?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let operation = async {
        #[cfg(unix)]
        let wait = async {
            super::observe_exit(child.id().expect("spawned child")).await?;
            group.terminate();
            child.wait().await
        };
        #[cfg(windows)]
        let wait = async {
            let status = child.wait().await?;
            tokio::time::timeout(super::COMMAND_CLEANUP_TIMEOUT, group.terminate_and_wait())
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "stdin command job cleanup timed out",
                    )
                })??;
            Ok::<_, io::Error>(status)
        };
        let feed = async {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, io::Error>(())
        };
        let (status, (), stdout, stderr) = tokio::try_join!(
            wait,
            feed,
            complete(stdout, output_limit),
            complete(stderr, output_limit)
        )?;
        Ok::<_, io::Error>(std::process::Output {
            status,
            stdout,
            stderr,
        })
    };
    let result = tokio::time::timeout(timeout, operation).await;
    if !matches!(&result, Ok(Ok(_))) {
        tokio::time::timeout(super::COMMAND_CLEANUP_TIMEOUT, async {
            #[cfg(unix)]
            group.terminate();
            #[cfg(windows)]
            group.terminate_and_wait().await?;
            child.wait().await?;
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "stdin command cleanup timed out")
        })??;
    }
    result.map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "stdin command execution deadline exceeded",
        )
    })?
}

fn fixed_program(
    executable: &Path,
    arguments: &[String],
    sandbox: &SandboxSpec,
) -> io::Result<Command> {
    if !sandbox.policy.is_enforcing() {
        let mut process = Command::new(executable);
        process.args(arguments);
        return Ok(process);
    }
    #[cfg(unix)]
    {
        // The existing sandbox API wraps a shell. Only this adapter's argv is
        // serialized, and every value is a single literal shell word.
        let executable = executable.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "executable is not UTF-8")
        })?;
        let command = std::iter::once(executable)
            .chain(arguments.iter().map(String::as_str))
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(argv) = sandbox.command_line("/bin/sh", &format!("exec {command}")) {
            let mut process = Command::new(&argv[0]);
            process.args(&argv[1..]);
            return Ok(process);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "requested OS sandbox is unavailable; fixed program was not started",
    ))
}

#[cfg(unix)]
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn complete(mut stream: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        if count > limit.saturating_sub(output.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fixed program output exceeded byte limit; entire result discarded",
            ));
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn raw_capture_preserves_binary_and_rejects_overflow_instead_of_returning_tail() {
        let data = [b'[', 0, 0xff, b']'];
        assert_eq!(complete(data.as_slice(), data.len()).await.unwrap(), data);
        assert_eq!(
            complete(data.as_slice(), 3).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            complete(b"x".as_slice(), 0).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(complete(b"".as_slice(), 0).await.unwrap().is_empty());
    }

    #[cfg(unix)]
    async fn run(
        script: &str,
        input: &[u8],
        limit: usize,
        timeout: Duration,
    ) -> io::Result<std::process::Output> {
        run_capture_stdin(
            Path::new("/bin/sh"),
            &["-c".into(), script.into()],
            &std::env::temp_dir().canonicalize()?,
            &SandboxSpec::new(crate::sandbox::SandboxPolicy::Off, []),
            input,
            timeout,
            limit,
        )
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_capture_feeds_stdin_concurrently_and_preserves_exit_status_and_stderr() {
        let input = vec![b'x'; 512 * 1024];
        let output = run(
            "cat; printf diagnostic >&2; exit 7",
            &input,
            input.len(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, input);
        assert_eq!(output.stderr, b"diagnostic");
        assert_eq!(output.status.code(), Some(7));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_capture_rejects_stdout_and_stderr_overflow_and_times_out() {
        for script in ["printf 12345", "printf 12345 >&2"] {
            assert_eq!(
                run(script, b"", 4, Duration::from_secs(2))
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(
            run("sleep 30", b"", 4, Duration::from_millis(30))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn raw_capture_cancellation_kills_descendants() {
        let directory =
            std::env::temp_dir().join(format!("willdeep-stdin-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let started = directory.join("started");
        let escaped = directory.join("escaped");
        let script = format!(
            "(sleep 1; printf escaped > {}) & printf ready > {}; wait",
            quote(escaped.to_str().unwrap()),
            quote(started.to_str().unwrap())
        );
        let mut running = Box::pin(run(&script, b"", 100, Duration::from_secs(30)));
        tokio::select! {
            result = &mut running => panic!("unexpected completion: {result:?}"),
            _ = async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !started.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
                }).await.unwrap();
            } => {}
        }
        // Drop the owned future, not just a Pin<&mut Future> reborrow.
        drop(running);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(
            !escaped.exists(),
            "a cancelled command left a live descendant"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
