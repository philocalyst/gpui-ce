use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use wait_timeout::ChildExt;

pub(crate) const OUTPUT_LIMIT: usize = 256 * 1024;

pub(crate) struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

pub(crate) fn run(
    program: &str,
    args: &[&std::ffi::OsStr],
    cwd: &Path,
    target_dir: &Path,
    timeout: Duration,
) -> std::io::Result<ProcessOutput> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_reader = thread::spawn(move || read_capped(stdout));
    let err_reader = thread::spawn(move || read_capped(stderr));

    let timed_out = match child.wait_timeout(timeout)? {
        Some(_) => false,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            true
        }
    };
    let status = child.try_wait()?.or_else(|| child.wait().ok());
    let (stdout, stdout_truncated) = out_reader.join().unwrap_or_default();
    let (stderr, stderr_truncated) = err_reader.join().unwrap_or_default();
    let mut stdout = stdout;
    let mut stderr = stderr;
    if stdout_truncated {
        stdout.extend_from_slice(b"\n[cargo-impact: stdout truncated at 256 KiB]\n");
    }
    if stderr_truncated {
        stderr.extend_from_slice(b"\n[cargo-impact: stderr truncated at 256 KiB]\n");
    }
    let code = status.and_then(|status| status.code());
    Ok(ProcessOutput {
        success: !timed_out && code == Some(0),
        code,
        stdout,
        stderr,
        timed_out,
    })
}

fn read_capped(mut reader: impl Read) -> (Vec<u8>, bool) {
    let mut kept = Vec::with_capacity(OUTPUT_LIMIT);
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(remaining)]);
                truncated |= read > remaining;
            }
        }
    }
    (kept, truncated)
}

pub(crate) fn write_log(stdout: &[u8], stderr: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(stdout.len() + stderr.len());
    let _ = bytes.write_all(stdout);
    if !stderr.is_empty() {
        let _ = bytes.write_all(b"\n--- stderr ---\n");
        let _ = bytes.write_all(stderr);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
