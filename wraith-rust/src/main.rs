use clap::{Parser, Subcommand};
use wraith_capturer::{capturer::Capturer, error::Result, ptrace_ops::ProcessLock, utils};

#[derive(Parser)]
#[command(
    name    = "wraith-capturer",
    about   = "Wraith — capture and restore running process state",
    version,
    long_about = "Wraith freezes a process via ptrace, captures its register and memory state,\n\
                  and serializes it for cross-machine restore.\n\n\
                  Phase 1 captures registers only. Memory capture is added in Phase 2."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Freeze a process and write its state to a snapshot file.
    Capture {
        /// PID of the target process.
        #[arg(short, long)]
        pid: i32,

        /// Output path for the snapshot (bincode format, Phase 2 upgrades to Protobuf).
        #[arg(short, long, default_value = "snapshot.bin")]
        output: String,
    },

    /// Resume a frozen process (emergency rollback — use if migration fails mid-flight).
    ///
    /// Attaches to the process and immediately detaches, causing it to resume.
    /// The process must be in ptrace-stop state (status 'T' in ps output).
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
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    let result = match cli.command {
        Command::Capture { pid, output } => cmd_capture(pid, &output),
        Command::Resume { pid }          => cmd_resume(pid),
        Command::Inspect { snapshot }    => cmd_inspect(&snapshot),
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

    println!("Snapshot saved: {}", output);
    println!("  pid      : {}", snapshot.pid);
    println!("  name     : {}", snapshot.process_name);
    println!("  arch     : {}", snapshot.arch);
    println!("  kernel   : {}", snapshot.kernel_version);
    println!("  rip      : {:#018x}", snapshot.registers.rip);
    println!("  rsp      : {:#018x}", snapshot.registers.rsp);
    println!("  fpu bytes: {}", snapshot.registers.fpu_state.len());
    Ok(())
}

fn cmd_resume(pid: i32) -> Result<()> {
    if !utils::pid_exists(pid) {
        return Err(wraith_capturer::error::err_process_not_found(pid));
    }
    // Attach + immediately detach → resumes a process that is in ptrace-stop.
    let mut lock = ProcessLock::attach(pid)?;
    lock.detach()?;
    println!("Process {} resumed.", pid);
    Ok(())
}

fn cmd_inspect(path: &str) -> Result<()> {
    let s = Capturer::load(path)?;
    println!("Snapshot: {}", path);
    println!("  pid            : {}", s.pid);
    println!("  name           : {}", s.process_name);
    println!("  arch           : {}", s.arch);
    println!("  kernel         : {}", s.kernel_version);
    println!("  captured_at_ns : {}", s.captured_at_ns);
    println!();
    println!("  Registers:");
    println!("    rip    {:#018x}    rsp    {:#018x}", s.registers.rip, s.registers.rsp);
    println!("    rax    {:#018x}    rbx    {:#018x}", s.registers.rax, s.registers.rbx);
    println!("    rcx    {:#018x}    rdx    {:#018x}", s.registers.rcx, s.registers.rdx);
    println!("    rdi    {:#018x}    rsi    {:#018x}", s.registers.rdi, s.registers.rsi);
    println!("    rbp    {:#018x}    rflags {:#018x}", s.registers.rbp, s.registers.rflags);
    println!("    r8     {:#018x}    r9     {:#018x}", s.registers.r8,  s.registers.r9);
    println!("    r10    {:#018x}    r11    {:#018x}", s.registers.r10, s.registers.r11);
    println!("    r12    {:#018x}    r13    {:#018x}", s.registers.r12, s.registers.r13);
    println!("    r14    {:#018x}    r15    {:#018x}", s.registers.r14, s.registers.r15);
    println!("    cs {:#x}  ss {:#x}  ds {:#x}  es {:#x}  fs {:#x}  gs {:#x}",
        s.registers.cs, s.registers.ss, s.registers.ds,
        s.registers.es, s.registers.fs, s.registers.gs);
    println!("    fs_base {:#018x}    gs_base {:#018x}",
        s.registers.fs_base, s.registers.gs_base);
    println!("  FPU state: {} bytes", s.registers.fpu_state.len());
    Ok(())
}
