// wraith-restorer — restore a process snapshot on the destination machine.
//
// The Python orchestrator (Phase 5) invokes this binary after the Go receiver
// has written the snapshot to disk. It:
//   1. Reads the Protobuf snapshot from the path given by --snapshot.
//   2. Forks a stub child, injects the snapshot address space via ptrace.
//   3. Restores register state and file descriptors.
//   4. Detaches — the restored process runs as a normal OS process.
//
// Usage:
//   wraith-restorer --snapshot /tmp/received.pb [--strict-fds]
//
// Exit codes:
//   0  — restored successfully
//   1  — validation or I/O error (snapshot not applied, no orphan process)
//   2  — restore partially failed (process was started but may be degraded)

use clap::Parser;
use wraith_capturer::{capturer::Capturer, error::Result, restorer::ProcessRestorer};

#[derive(Parser)]
#[command(
    name  = "wraith-restorer",
    about = "Wraith — restore a captured process snapshot to a new process",
    version,
    long_about = "Reads a Protobuf snapshot (written by wraith-capturer or the Go receiver),\n\
                  forks a stub process, and reconstructs the address space via ptrace syscall\n\
                  injection. The restored process resumes execution from the exact instruction\n\
                  pointer captured on the source machine.\n\n\
                  Must be run as root (or with CAP_SYS_PTRACE) on Linux x86-64."
)]
struct Cli {
    /// Path to the Protobuf snapshot file.
    #[arg(short, long)]
    snapshot: String,

    /// Fail if any file descriptor cannot be restored.
    ///
    /// By default, FD restoration failures are logged as warnings and the
    /// process is started anyway. Pass --strict-fds to treat them as fatal.
    #[arg(long, default_value_t = false)]
    strict_fds: bool,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    match run(&cli) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    // Load snapshot.
    log::info!("Loading snapshot from {}", cli.snapshot);
    let snapshot = Capturer::load(&cli.snapshot)?;

    let meta = snapshot.metadata.as_ref();
    log::info!(
        "Snapshot: pid={} arch={} regions={} fds={}",
        snapshot.pid,
        snapshot.arch,
        snapshot.memory_regions.len(),
        snapshot.file_descriptors.len()
    );
    if let Some(m) = meta {
        log::info!(
            "  source: {} ({}), captured at ns={}",
            m.machine_hostname, m.process_name, snapshot.captured_at_ns
        );
    }

    let total_mb: u64 = snapshot.memory_regions.iter().map(|r| r.size_bytes).sum::<u64>()
        / 1024 / 1024;
    log::info!("  memory: {} MB across {} regions", total_mb, snapshot.memory_regions.len());

    // Restore.
    log::info!("Starting restore...");
    let restorer = ProcessRestorer::new(snapshot);
    let result   = restorer.restore()?;

    // Report.
    println!("\nRestore complete:");
    println!("  restored pid  : {}", result.pid);
    println!("  regions mapped: {}", result.regions_ok);
    println!(
        "  fds restored  : {} / {} (skipped: {}, failed: {})",
        result.fd_report.restored.len(),
        result.fd_report.restored.len()
            + result.fd_report.skipped.len()
            + result.fd_report.failed.len(),
        result.fd_report.skipped.len(),
        result.fd_report.failed.len()
    );

    if !result.fd_report.skipped.is_empty() {
        println!("\n  Skipped FDs (non-fatal):");
        for outcome in &result.fd_report.skipped {
            if let wraith_capturer::fd_restore::FdOutcome::Skipped { fd_num, reason } = outcome {
                println!("    fd {}: {}", fd_num, reason);
            }
        }
    }

    if result.fd_report.has_failures() {
        eprintln!("\n  WARNING: {} FD(s) failed to restore:", result.fd_report.failed.len());
        for outcome in &result.fd_report.failed {
            if let wraith_capturer::fd_restore::FdOutcome::Failed { fd_num, error } = outcome {
                eprintln!("    fd {}: {}", fd_num, error);
            }
        }
        if cli.strict_fds {
            eprintln!("Aborting due to --strict-fds (process {} is still running)", result.pid);
            // Kill the orphaned process before exiting.
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(result.pid),
                nix::sys::signal::Signal::SIGKILL,
            );
            std::process::exit(2);
        }
    }

    println!("\nProcess {} is now running on this machine.", result.pid);
    Ok(())
}
