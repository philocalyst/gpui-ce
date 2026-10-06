//! Drain both pipes continuously, retain bounded logs, and parse diagnostics after log limits.

use std::{
    collections::{BTreeSet, VecDeque},
    io::{self, Read},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::model::CompilerDiagnostic;

const LOG_LIMIT: usize = 256 * 1024;
const VALUE_LIMIT: usize = 16 * 1024 * 1024;
const DIAGNOSTIC_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Capture {
    Bytes,
    Cargo,
}

#[derive(Default)]
pub(crate) struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub log: String,
    pub timed_out: bool,
    pub log_truncated: bool,
    pub data_truncated: bool,
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub artifacts: BTreeSet<String>,
}

pub(crate) fn run(
    command: &mut Command,
    timeout: Duration,
    capture: Capture,
) -> io::Result<ProcessOutput> {
    if !cfg!(unix) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "build workers require a Unix host for bounded process-tree termination",
        ));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr pipe"))?;
    #[cfg(unix)]
    {
        if let Err(error) = nonblocking(&stdout).and_then(|_| nonblocking(&stderr)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let out_stop = stopped.clone();
    let err_stop = stopped.clone();
    let out_reader = thread::spawn(move || drain(stdout, capture, &out_stop));
    let err_reader = thread::spawn(move || drain(stderr, Capture::Cargo, &err_stop));
    let wait = child.wait_timeout(timeout);
    let timed_out = matches!(wait, Ok(None));
    // Background grandchildren must not hold log pipes open, even on normal parent exit.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    if timed_out || wait.is_err() {
        let _ = child.kill();
    }
    let status = child.wait();
    stopped.store(true, Ordering::Release);
    let stdout = out_reader
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"))??;
    let stderr = err_reader
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"))??;
    wait?;
    let status = status?;
    Ok(ProcessOutput {
        success: status.success() && !timed_out,
        code: status.code(),
        stdout: stdout.value,
        log: format!("{}\n{}", stdout.log.text(), stderr.log.text()),
        timed_out,
        log_truncated: stdout.log.truncated || stderr.log.truncated,
        data_truncated: stdout.truncated || stderr.truncated,
        diagnostics: stdout
            .diagnostics
            .into_iter()
            .chain(stderr.diagnostics)
            .collect(),
        artifacts: stdout
            .artifacts
            .into_iter()
            .chain(stderr.artifacts)
            .collect(),
    })
}

#[derive(Default)]
struct Stream {
    value: Vec<u8>,
    log: Tail,
    truncated: bool,
    diagnostics: Vec<CompilerDiagnostic>,
    artifacts: BTreeSet<String>,
    diagnostic_bytes: usize,
}

#[derive(Default)]
struct Tail {
    bytes: VecDeque<u8>,
    truncated: bool,
}
impl Tail {
    fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        let excess = self.bytes.len().saturating_sub(LOG_LIMIT);
        self.truncated |= excess > 0;
        self.bytes.drain(..excess);
    }
    fn text(&self) -> String {
        let bytes: Vec<_> = self.bytes.iter().copied().collect();
        let prefix = if self.truncated {
            "[earlier output truncated]\n"
        } else {
            ""
        };
        format!("{prefix}{}", String::from_utf8_lossy(&bytes))
    }
}

fn drain(mut reader: impl Read, capture: Capture, stopped: &AtomicBool) -> io::Result<Stream> {
    let mut stream = Stream::default();
    let mut buffer = [0; 8192];
    let mut line = Vec::new();
    let mut discard_line = false;
    let mut stop_time = None;
    loop {
        if stopped.load(Ordering::Acquire) {
            let stop = stop_time.get_or_insert_with(Instant::now);
            if stop.elapsed() > Duration::from_millis(200) {
                // A detached descendant may retain the pipe. Do not let it hold the scan open.
                stream.truncated = true;
                break;
            }
        }
        let size = match reader.read(&mut buffer) {
            Ok(size) => size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => return Err(error),
        };
        if size == 0 {
            break;
        }
        match capture {
            Capture::Bytes => {
                stream.log.append(&buffer[..size]);
                let remaining = VALUE_LIMIT.saturating_sub(stream.value.len());
                stream
                    .value
                    .extend_from_slice(&buffer[..size.min(remaining)]);
                stream.truncated |= size > remaining;
            }
            Capture::Cargo => {
                for byte in &buffer[..size] {
                    if *byte == b'\n' {
                        if !discard_line {
                            stream.record(&line);
                        }
                        line.clear();
                        discard_line = false;
                    } else if !discard_line {
                        if line.len() == VALUE_LIMIT {
                            stream.truncated = true;
                            discard_line = true;
                            line.clear();
                        } else {
                            line.push(*byte);
                        }
                    }
                }
            }
        }
    }
    if matches!(capture, Capture::Cargo) && !line.is_empty() {
        stream.record(&line);
    }
    Ok(stream)
}

impl Stream {
    fn record(&mut self, line: &[u8]) {
        match serde_json::from_slice::<CargoMessage>(line) {
            Ok(CargoMessage::CompilerMessage {
                package_id,
                target,
                message,
            }) => {
                if self.diagnostic_bytes + line.len() > DIAGNOSTIC_LIMIT
                    || self.diagnostics.len() >= 512
                {
                    self.truncated = true;
                    return;
                }
                if matches!(
                    message.level,
                    cargo_metadata::diagnostic::DiagnosticLevel::Error
                        | cargo_metadata::diagnostic::DiagnosticLevel::Ice
                ) && let Some(rendered) = &message.rendered
                {
                    self.log.append(rendered.as_bytes());
                }
                self.diagnostic_bytes += line.len();
                self.diagnostics.push(CompilerDiagnostic {
                    package_id,
                    target,
                    package: None,
                    source_files: Default::default(),
                    diagnostic: *message,
                });
            }
            Ok(CargoMessage::CompilerArtifact { package_id }) => {
                if self.artifacts.len() < 100_000 {
                    self.artifacts.insert(package_id);
                } else {
                    self.truncated = true;
                }
            }
            Ok(CargoMessage::Other) => {}
            Err(_) => {
                let text = String::from_utf8_lossy(line);
                if ![
                    "Compiling ",
                    "Checking ",
                    "Finished ",
                    "Downloading ",
                    "Updating ",
                ]
                .iter()
                .any(|v| text.trim_start().starts_with(v))
                {
                    self.log.append(line);
                    self.log.append(b"\n");
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    CompilerMessage {
        package_id: String,
        #[serde(default)]
        target: Option<cargo_metadata::Target>,
        message: Box<cargo_metadata::diagnostic::Diagnostic>,
    },
    CompilerArtifact {
        package_id: String,
    },
    #[serde(other)]
    Other,
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_diagnostic_survives_progress_output_limit() {
        let mut bytes = vec![b'x'; LOG_LIMIT * 3];
        bytes.push(b'\n');
        bytes.extend_from_slice(br#"{"reason":"compiler-message","package_id":"consumer","message":{"message":"removed API","code":{"code":"E0425","explanation":null},"level":"error","spans":[],"children":[],"rendered":"error[E0425]"}}"#);
        let result = drain(&bytes[..], Capture::Cargo, &AtomicBool::new(false)).unwrap();
        assert!(result.log.truncated);
        assert!(!result.truncated);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].message, "removed API");
    }

    #[test]
    fn cancelled_reader_bounds_drain_even_if_a_detached_writer_keeps_its_pipe() {
        struct OpenPipe;
        impl Read for OpenPipe {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::WouldBlock.into())
            }
        }
        let started = Instant::now();
        let stream = drain(OpenPipe, Capture::Cargo, &AtomicBool::new(true)).unwrap();
        assert!(stream.truncated);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_grandchildren_holding_pipes() {
        let started = std::time::Instant::now();
        let output = run(
            Command::new("sh").args(["-c", "sleep 20 & wait"]),
            Duration::from_millis(100),
            Capture::Cargo,
        )
        .unwrap();
        assert!(output.timed_out);
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
