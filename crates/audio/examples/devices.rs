//! Read-only device query: never constructs or plays a recording stream.
fn main() {
    match speakmore_audio::device::list_inputs() {
        Ok(devices) => println!("{}", serde_json::to_string_pretty(&devices).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
