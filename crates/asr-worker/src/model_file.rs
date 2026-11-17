use crate::cancellation::Cancellation;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};
type EngineResult<T> = Result<T, &'static str>;

/// The ModelManager must establish trust in the manifest before issuing a load.
/// Here we additionally check actual bytes before parsing native model data.
pub fn verify_file(
    path: &str,
    expected_size: u64,
    expected_hash: &str,
    cancellation: &Cancellation,
) -> EngineResult<()> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("InvalidModelPath");
    }
    let meta = fs::symlink_metadata(path).map_err(|_| "ModelMissing")?;
    if !meta.is_file() || meta.len() != expected_size {
        return Err("ModelSizeMismatch");
    }
    let mut file = File::open(path).map_err(|_| "ModelUnreadable")?;
    let mut hasher = Sha256::new();
    let mut block = [0u8; 65536];
    let mut total = 0u64;
    loop {
        cancellation.check()?;
        let size = file.read(&mut block).map_err(|_| "ModelUnreadable")?;
        if size == 0 {
            break;
        }
        total += size as u64;
        if total > expected_size {
            return Err("ModelSizeMismatch");
        }
        hasher.update(&block[..size]);
    }
    if total != expected_size || format!("{:x}", hasher.finalize()) != expected_hash {
        return Err("ModelHashMismatch");
    }
    Ok(())
}
