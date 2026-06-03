use kani_mono::cli::run_cli;
use kani_mono::runtime::ReplayRuntime;

fn main() {
    let response = std::env::var("KANI_MONO_REPLAY_RESPONSE").unwrap_or_default();
    match run_cli(std::env::args().skip(1), ReplayRuntime::new(response)) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
