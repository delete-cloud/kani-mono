use kani_mono::cli::run_cli_stdio;
use kani_mono::runtime::ReplayRuntime;

fn main() {
    let response = std::env::var("KANI_MONO_REPLAY_RESPONSE").unwrap_or_default();
    match run_cli_stdio(
        std::env::args().skip(1),
        ReplayRuntime::new(response),
        std::io::stdin(),
        std::io::stdout(),
    ) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
