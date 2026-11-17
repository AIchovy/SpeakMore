mod backend;
mod cancellation;
mod engine;
mod model_file;
mod runtime;
mod whisper_cancellation;

fn main() {
    // Never print transcripts, dictionary text, paths or native diagnostic logs.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("WorkerPanic");
        runtime::terminate(70);
    }));
    backend::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|s| s != "--cpu") || args.len() > 1 {
        std::process::exit(64);
    }
    let gpu = args.is_empty() && (cfg!(feature = "metal") || cfg!(feature = "vulkan"));
    let backend = if gpu && cfg!(feature = "metal") {
        "metal"
    } else if gpu {
        "vulkan"
    } else {
        "cpu"
    };
    runtime::run(gpu, backend);
}
