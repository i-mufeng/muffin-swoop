// Console-subsystem binary on Windows: stdout, redirection and process waiting
// work without attaching a console to the graphical executable.
mod cli;
#[allow(dead_code)] // The GUI owns the server half; this binary only uses its client.
mod instance;

fn main() -> std::process::ExitCode {
    cli::attach_parent_console();
    std::process::ExitCode::from(cli::run(std::env::args_os().skip(1)))
}
