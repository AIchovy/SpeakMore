mod cancellation;
#[path = "sensevoice_engine.rs"]
mod engine;
mod model_file;
mod runtime;

fn main() {
    std::panic::set_hook(Box::new(|_| runtime::terminate(70)));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 1 || args.iter().any(|s| s != "--cpu") {
        std::process::exit(64);
    }
    runtime::run(false, "cpu");
}
