//! Process entry point; handlers return a consistent compatibility exit status.

mod cli;
mod setup;

fn main() {
    let exit = match cli::run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo-impact: {error}");
            2
        }
    };
    std::process::exit(exit);
}
