use clap::{Parser, Subcommand};
use wraith_capturer::{capturer::Capturer, error::Result, ptrace_ops::ProcessLock, utils};

#[derive(Parser)]
#[command(
    name    = "wraith-capturer",
    about   = "Wraith — capture and restore running process state",
    version,
    long_about = "Freezes a process via ptrace, captures registers + memory + file descriptors,\n\
                  and serializes the state to a Protobuf snapshot file for cross-machine restore.\n\n\
                  The process is resumed automatically on any error (rollback is guaranteed)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Freeze a process and write its complete state to a snapshot file.
    Capture {
        /// PID of the target process.
        #[arg(short, long)]
        pid: i32,

        /// Output path for the snapshot (Protobuf binary format).
        #[arg(short, long, default_value = "snapshot.pb")]
        output: String,
    },

    /// Resume a frozen process (emergency rollback).
    ///
    /// Use this if a migration fails and the source process is stuck in
    /// ptrace-stop state (shows as 'T' in `ps` output).
    Resume {
        /// PID of the frozen process.
        #[arg(short, long)]
        pid: i32,
    },

    /// Print the contents of a snapshot file.
    Inspect {
        /// Path to the snapshot file.
        #[arg(short, long)]
        snapshot: String,

        /// Show memory region list.
        #[arg(long)]
        regions: bool,

        /// Show file descriptor list.
        #[arg(long)]
        fds: bool,
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    let result = match cli.command {
        Command::Capture { pid, output }                    => cmd_capture(pid, &output),
        Command::Resume  { pid }                            => cmd_resume(pid),
        Command::Inspect { snapshot, regions, fds }        => cmd_inspect(&snapshot, regions, fds),
    };

    if let Err(e) = result {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn cmd_capture(pid: i32, output: &str) -> Result<()> {
    let capturer = Capturer::new(pid);
    let snapshot = capturer.capture()?;
    Capturer::save(&snapshot, output)?;

    let regs = snapshot.registers.as_ref();
    let meta = snapshot.metadata.as_ref();

    println!("Snapshot saved: {}", output);
    println!("  pid        : {}", snapshot.pid);
    println!("  name       : {}", meta.map(|m| m.process_name.as_str()).unwrap_or("?"));
    println!("  arch       : {}", snapshot.arch);
    println!("  kernel     : {}", snapshot.kernel_version);
    println!("  rip        : {:#018x}", regs.map(|r| r.rip).unwrap_or(0));
    println!("  rsp        : {:#018x}", regs.map(|r| r.rsp).unwrap_or(0));
    println!("  regions    : {}", snapshot.memory_regions.len());
    println!("  fds        : {}", snapshot.file_descriptors.len());

    let total_mb: u64 = snapshot.memory_regions.iter().map(|r| r.size_bytes).sum::<u64>() / 1024 / 1024;
    println!("  memory     : {} MB captured", total_mb);

    Ok(())
}

fn cmd_resume(pid: i32) -> Result<()> {
    if !utils::pid_exists(pid) {
        return Err(wraith_capturer::error::err_process_not_found(pid));
    }
    let mut lock = ProcessLock::attach(pid)?;
    lock.detach()?;
    println!("Process {} resumed.", pid);
    Ok(())
}

fn cmd_inspect(path: &str, show_regions: bool, show_fds: bool) -> Result<()> {
    let s = Capturer::load(path)?;
    let regs = s.registers.as_ref();
    let meta = s.metadata.as_ref();

    println!("Snapshot: {}", path);
    println!("  pid            : {}", s.pid);
    println!("  arch           : {}", s.arch);
    println!("  kernel         : {}", s.kernel_version);
    println!("  captured_at_ns : {}", s.captured_at_ns);
    println!("  version        : {}", s.snapshot_version);

    if let Some(m) = meta {
        println!("  process name   : {}", m.process_name);
        println!("  hostname       : {}", m.machine_hostname);
        println!("  vmsize         : {} MB", m.virtual_memory_bytes / 1024 / 1024);
        println!("  vmrss          : {} MB", m.resident_memory_bytes / 1024 / 1024);
    }

    if let Some(r) = regs {
        println!();
        println!("  Registers:");
        println!("    rip    {:#018x}    rsp    {:#018x}", r.rip, r.rsp);
        println!("    rax    {:#018x}    rbx    {:#018x}", r.rax, r.rbx);
        println!("    rcx    {:#018x}    rdx    {:#018x}", r.rcx, r.rdx);
        println!("    rdi    {:#018x}    rsi    {:#018x}", r.rdi, r.rsi);
        println!("    rbp    {:#018x}    rflags {:#018x}", r.rbp, r.rflags);
        println!("    r8     {:#018x}    r9     {:#018x}", r.r8,  r.r9);
        println!("    r10    {:#018x}    r11    {:#018x}", r.r10, r.r11);
        println!("    r12    {:#018x}    r13    {:#018x}", r.r12, r.r13);
        println!("    r14    {:#018x}    r15    {:#018x}", r.r14, r.r15);
        println!("    cs {:#x}  ss {:#x}  fs_base {:#x}",
            r.cs, r.ss, r.fs_base);
        println!("    fpu: {} bytes", r.fpu_state.len());
    }

    println!();
    println!("  Memory regions : {}", s.memory_regions.len());
    let total_mb: u64 = s.memory_regions.iter().map(|r| r.size_bytes).sum::<u64>() / 1024 / 1024;
    println!("  Total memory   : {} MB", total_mb);

    if show_regions {
        println!();
        println!("  {:<20} {:<10} {:<6} {:<12} {}", "Start", "Size", "Perms", "Type", "Backing");
        for r in &s.memory_regions {
            println!(
                "  {:#018x}  {:<10} {:<6} {:<12} {}",
                r.start_addr,
                format!("{} KB", r.size_bytes / 1024),
                r.permissions,
                r.region_type,
                if r.backing_file.is_empty() { "(anon)" } else { &r.backing_file }
            );
        }
    }

    println!();
    println!("  File descriptors: {}", s.file_descriptors.len());

    if show_fds {
        println!();
        println!("  {:<6} {:<10} {:<10} {}", "FD", "Type", "Offset", "Path");
        for f in &s.file_descriptors {
            println!("  {:<6} {:<10} {:<10} {}", f.fd_num, f.fd_type, f.file_offset, f.path);
        }
    }

    Ok(())
}
