#![windows_subsystem = "windows"]

fn main() {
    if let Some(code) = window_switcher::utils::run_startup_helper() {
        std::process::exit(code);
    }
    if let Err(error) = window_switcher::run() {
        // A replacement reports failure through its owned pipe/process status;
        // a modal startup dialog must not keep the failed candidate alive.
        if !std::env::args_os().any(|argument| argument == "--restart-child") {
            window_switcher::alert!("{error:#}");
        }
        std::process::exit(1);
    }
}
