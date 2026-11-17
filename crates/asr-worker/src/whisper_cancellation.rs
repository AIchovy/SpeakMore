use crate::cancellation::Cancellation;
use std::ffi::c_void;
use whisper_rs::{FullParams, WhisperError, WhisperState};

unsafe extern "C" fn abort(data: *mut c_void) -> bool {
    // SAFETY: decode owns the Box until full returns; C only reads this atomic.
    // This callback does not allocate, lock, panic or access the WhisperState.
    unsafe { &*(data.cast::<Cancellation>()) }.reason() != 0
}

pub fn decode(
    state: &mut WhisperState,
    mut params: FullParams<'_, '_>,
    samples: &[f32],
    cancellation: &Cancellation,
) -> Result<(), WhisperError> {
    let mut owned = Box::new(cancellation.clone());
    // SAFETY: stable allocation, exact callback data type, no escaping params;
    // whisper_full_with_state completes its backend work before returning.
    unsafe {
        params.set_abort_callback(Some(abort));
        params.set_abort_callback_user_data((&mut *owned as *mut Cancellation).cast());
    }
    let result = state.full(params, samples);
    drop(owned);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::{TIMEOUT, USER};
    #[test]
    fn callback_observes_atomic_and_releases_ownership() {
        let cancellation = Cancellation::default();
        let mut owned = Box::new(cancellation.clone());
        let ptr = (&mut *owned as *mut Cancellation).cast();
        assert!(!unsafe { abort(ptr) });
        cancellation.cancel(USER);
        assert!(unsafe { abort(ptr) });
        cancellation.cancel(TIMEOUT);
        assert_eq!(cancellation.reason(), USER);
        drop(owned);
        assert_eq!(cancellation.owner_count(), 1);
    }
}
