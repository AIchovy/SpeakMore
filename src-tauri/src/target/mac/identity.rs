//! Bind a target to its bundle ID, PID and launch time. TextEdit additionally
//! retains its existing Apple signature check; other apps use live AX capability checks.
use super::{ProbeResult, Query, Reason};
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};

const BUNDLE: &str = "com.apple.TextEdit";
const REQUIREMENT: &str = "identifier \"com.apple.TextEdit\" and anchor apple";

unsafe extern "C" {
    // libproc.h; PROC_PIDTBSDINFO fills `BsdInfo`.
    fn proc_pidinfo(
        pid: i32,
        flavor: i32,
        arg: u64,
        buffer: *mut std::ffi::c_void,
        size: i32,
    ) -> i32;
}
// sys/proc_info.h struct proc_bsdinfo.
#[repr(C)]
struct BsdInfo {
    flags: u32,
    status: u32,
    xstatus: u32,
    pid: u32,
    ppid: u32,
    ids: [u32; 6],
    reserved: u32,
    comm: [u8; 16],
    name: [u8; 32],
    nfiles: u32,
    pgid: u32,
    pjobc: u32,
    tdev: u32,
    tpgid: u32,
    nice: i32,
    start_sec: u64,
    start_usec: u64,
}
const PROC_PIDTBSDINFO: i32 = 3;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    // Security permits a dynamic SecCodeRef here and obtains the corresponding
    // static information. Check validity first; CopySigningInformation alone
    // does not authenticate its result. See the SDK's SecCode.h.
    fn SecCodeCopySigningInformation(code: CFTypeRef, flags: u32, out: *mut CFTypeRef) -> i32;
    static kSecCodeInfoIdentifier: CFStringRef;
    static kSecCodeInfoPList: CFStringRef;
    static kSecCodeInfoUnique: CFStringRef;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    process: Process,
    code: Option<CodeIdentity>,
}
impl Identity {
    pub(super) fn pid(&self) -> i32 {
        self.process.pid
    }
    /// One process launch: PID plus start time, never a reused PID.
    pub(super) fn key(&self) -> (i32, u64) {
        (self.process.pid, self.process.birth)
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Process {
    pid: i32,
    birth: u64,
    bundle: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct CodeIdentity {
    version: String,
    build: String,
    unique: Vec<u8>,
}

fn process(q: &Query, pid: i32) -> ProbeResult<Process> {
    q.time()?;
    if pid <= 0 {
        return Err(Reason::TargetUnavailable);
    }
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or(Reason::TargetUnavailable)?;
    if running.isTerminated() || running.processIdentifier() != pid {
        return Err(Reason::TargetUnavailable);
    }
    let bundle = crate::applications::mac::identity(&running).ok_or(Reason::UnsupportedTarget)?;
    // Never deliver into our own settings/search fields.
    if pid == std::process::id() as i32 || bundle.starts_with("com.speakmore.") {
        return Err(Reason::UnsupportedTarget);
    }
    // Apps not launched by LaunchServices (a terminal, a debugger) have no
    // launch date; the kernel start time identifies their launch instead.
    let date = match running.launchDate() {
        Some(date) => date.timeIntervalSince1970(),
        None => started(pid)?,
    };
    if !date.is_finite() || date <= 0.0 {
        return Err(Reason::TargetUnavailable);
    }
    q.time()?;
    Ok(Process {
        pid,
        birth: date.to_bits(),
        bundle,
    })
}

fn started(pid: i32) -> ProbeResult<f64> {
    let mut info: BsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<BsdInfo>() as i32;
    let read = unsafe { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size || info.pid != pid as u32 || info.start_sec == 0 {
        return Err(Reason::TargetUnavailable);
    }
    Ok(info.start_sec as f64 + info.start_usec as f64 / 1e6)
}

thread_local! {
    // The last validated signature on this thread, bound to one launch.
    static VALIDATED: std::cell::RefCell<Option<((i32, u64), CodeIdentity)>> =
        const { std::cell::RefCell::new(None) };
}
/// Require a new signature validation for the next capture on this thread.
pub(super) fn forget() {
    VALIDATED.set(None);
}

pub(super) fn capture(q: &Query, pid: i32) -> ProbeResult<Identity> {
    let before = process(q, pid)?;
    if before.bundle != "com.apple.textedit" {
        return Ok(Identity {
            process: before,
            code: None,
        });
    }
    let key = (pid, before.birth);
    let known =
        VALIDATED.with_borrow(|v| v.as_ref().filter(|(k, _)| *k == key).map(|v| v.1.clone()));
    if let Some(code) = known {
        return Ok(Identity {
            process: before,
            code: Some(code),
        });
    }
    let code = signed_code(q, pid)?;
    // Re-fetch NSRunningApplication, rather than reusing cached properties.
    // A terminate/relaunch or PID reuse during validation cannot bind this
    // signature to a different launch. The AX observer supplies focus changes.
    if process(q, pid)? != before {
        return Err(Reason::TargetChanged);
    }
    VALIDATED.set(Some((key, code.clone())));
    Ok(Identity {
        process: before,
        code: Some(code),
    })
}

/// Last-moment check off the probe thread: Workspace state only, no AX.
pub(super) fn frontmost_is(pid: i32) -> bool {
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .is_some_and(|app| !app.isTerminated() && app.processIdentifier() == pid)
    })
}

fn frontmost_pid(q: &Query) -> ProbeResult<i32> {
    super::note_step("FrontmostApplication");
    q.time()?;
    let app = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .ok_or(Reason::TargetUnavailable)?;
    let pid = app.processIdentifier();
    if app.isTerminated() || pid <= 0 {
        return Err(Reason::TargetUnavailable);
    }
    q.time()?;
    Ok(pid)
}

/// Callers bind the AX application and then `require_frontmost` once more.
pub(super) fn frontmost(q: &Query) -> ProbeResult<Identity> {
    capture(q, frontmost_pid(q)?)
}

pub(super) fn require_frontmost(q: &Query, expected: &Identity) -> ProbeResult<()> {
    let pid = frontmost_pid(q)?;
    // Check PID before querying another process, then launch time and bundle
    // to reject PID reuse. Workspace generation also catches switching away
    // and back during a dictation.
    if pid != expected.pid() || process(q, pid)? != expected.process {
        return Err(Reason::TargetChanged);
    }
    Ok(())
}

fn signed_code(q: &Query, pid: i32) -> ProbeResult<CodeIdentity> {
    q.time()?;
    if pid <= 0 {
        return Err(Reason::TargetUnavailable);
    }
    let requirement: SecRequirement = REQUIREMENT.parse().map_err(|_| Reason::TargetUnavailable)?;
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid);
    // A fresh kernel guest per validation; a result is reused only for the same
    // launch (PID and start time). Never authenticate only a bundle path
    // supplied by another application.
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE);
    q.time()?;
    let code = code.map_err(|_| Reason::TargetUnavailable)?;
    let checked = code.check_validity(Flags::NO_NETWORK_ACCESS, &requirement);
    q.time()?;
    checked.map_err(|_| Reason::UnsupportedTarget)?;
    let mut out = std::ptr::null();
    let status = unsafe { SecCodeCopySigningInformation(code.as_CFTypeRef(), 0, &mut out) };
    // Adopt any returned object even on an error; every copy is released and
    // CF types are checked before interpreting dictionary values.
    let info = (!out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) });
    q.time()?;
    if status != 0 {
        return Err(Reason::TargetUnavailable);
    }
    let info = info
        .and_then(|info| info.downcast_into::<CFDictionary>())
        .ok_or(Reason::TargetUnavailable)?;
    let identity = metadata(&info)?;
    q.time()?;
    Ok(identity)
}

fn value(info: &CFDictionary, key: CFStringRef) -> ProbeResult<CFType> {
    // `find` borrows the value; wrap under get rule retains it across the
    // temporary ItemRef. Both the key and values remain framework CF objects.
    let pointer = info.find(key.cast()).ok_or(Reason::TargetUnavailable)?;
    if pointer.is_null() {
        return Err(Reason::TargetUnavailable);
    }
    Ok(unsafe { CFType::wrap_under_get_rule(*pointer) })
}

fn string(info: &CFDictionary, key: CFStringRef, limit: isize) -> ProbeResult<String> {
    let text = value(info, key)?
        .downcast_into::<CFString>()
        .ok_or(Reason::TargetUnavailable)?;
    if text.char_len() <= 0 || text.char_len() > limit {
        return Err(Reason::TargetUnavailable);
    }
    let text = text.to_string();
    if !text
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
    {
        return Err(Reason::TargetUnavailable);
    }
    Ok(text)
}

fn metadata(info: &CFDictionary) -> ProbeResult<CodeIdentity> {
    if string(info, unsafe { kSecCodeInfoIdentifier }, 128)? != BUNDLE {
        return Err(Reason::UnsupportedTarget);
    }
    let plist = value(info, unsafe { kSecCodeInfoPList })?
        .downcast_into::<CFDictionary>()
        .ok_or(Reason::TargetUnavailable)?;
    if string(
        &plist,
        CFString::new("CFBundleIdentifier").as_concrete_TypeRef(),
        128,
    )? != BUNDLE
    {
        return Err(Reason::UnsupportedTarget);
    }
    let version = string(
        &plist,
        CFString::new("CFBundleShortVersionString").as_concrete_TypeRef(),
        64,
    )?;
    let build = string(
        &plist,
        CFString::new("CFBundleVersion").as_concrete_TypeRef(),
        64,
    )?;
    let unique = value(info, unsafe { kSecCodeInfoUnique })?
        .downcast_into::<CFData>()
        .ok_or(Reason::TargetUnavailable)?;
    // The SDK does not promise one digest algorithm; keep a bounded binary
    // identity instead of parsing/displaying it or assuming a 20-byte SHA-1.
    if !(16..=64).contains(&unique.len()) {
        return Err(Reason::TargetUnavailable);
    }
    Ok(CodeIdentity {
        version,
        build,
        unique: unique.bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::number::CFNumber;
    use objc2::rc::autoreleasepool;
    use objc2_foundation::NSString;
    use std::time::{Duration, Instant};

    fn query() -> Query {
        Query::new(Instant::now() + Duration::from_millis(300))
    }

    // Rejection tests isolate the signing requirement from cold signature I/O
    // for the large test executable. Production and TextEdit budget checks
    // retain 300 ms; the deadline test above checks expiration separately.
    fn rejection_query() -> Query {
        Query::new(Instant::now() + Duration::from_secs(5))
    }

    fn fixture(identifier: &str, version: CFType, build: &str, unique: CFType) -> CFDictionary {
        let plist = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("CFBundleIdentifier"),
                CFString::new(BUNDLE).as_CFType(),
            ),
            (CFString::new("CFBundleShortVersionString"), version),
            (
                CFString::new("CFBundleVersion"),
                CFString::new(build).as_CFType(),
            ),
        ]);
        CFDictionary::from_CFType_pairs(&[
            (
                unsafe { CFString::wrap_under_get_rule(kSecCodeInfoIdentifier) },
                CFString::new(identifier).as_CFType(),
            ),
            (
                unsafe { CFString::wrap_under_get_rule(kSecCodeInfoPList) },
                plist.as_CFType(),
            ),
            (
                unsafe { CFString::wrap_under_get_rule(kSecCodeInfoUnique) },
                unique,
            ),
        ])
        .into_untyped()
    }

    #[test]
    fn metadata_requires_typed_bounded_signed_identity_and_versions() {
        let valid = || {
            fixture(
                BUNDLE,
                CFString::new("1.20").as_CFType(),
                "409",
                CFData::from_buffer(&[7; 20]).as_CFType(),
            )
        };
        let identity = metadata(&valid()).unwrap();
        assert_eq!(identity.version, "1.20");
        assert_eq!(identity.build, "409");
        assert_eq!(identity.unique, vec![7; 20]);
        for text in ["", "1.20\n", "1.20\0", "版本", &"a".repeat(65)] {
            let info = fixture(
                BUNDLE,
                CFString::new(text).as_CFType(),
                "409",
                CFData::from_buffer(&[7; 20]).as_CFType(),
            );
            assert!(metadata(&info).is_err());
        }
        let wrong_version = fixture(
            BUNDLE,
            CFNumber::from(120_i32).as_CFType(),
            "409",
            CFData::from_buffer(&[7; 20]).as_CFType(),
        );
        assert!(metadata(&wrong_version).is_err());
        let wrong_hash = fixture(
            BUNDLE,
            CFString::new("1.20").as_CFType(),
            "409",
            CFString::new("not a digest").as_CFType(),
        );
        assert!(metadata(&wrong_hash).is_err());
        for length in [0, 15, 65] {
            let info = fixture(
                BUNDLE,
                CFString::new("1.20").as_CFType(),
                "409",
                CFData::from_buffer(&vec![7; length]).as_CFType(),
            );
            assert!(metadata(&info).is_err());
        }
        let wrong_id = fixture(
            "org.example.TextEdit",
            CFString::new("1.20").as_CFType(),
            "409",
            CFData::from_buffer(&[7; 20]).as_CFType(),
        );
        assert_eq!(metadata(&wrong_id), Err(Reason::UnsupportedTarget));
        assert!(
            metadata(&CFDictionary::<CFString, CFType>::from_CFType_pairs(&[]).into_untyped())
                .is_err()
        );
        let mut other = metadata(&valid()).unwrap();
        other.build = "410".into();
        assert_ne!(identity, other);
        other = metadata(&valid()).unwrap();
        other.unique[0] ^= 1;
        assert_ne!(identity, other);
    }

    #[test]
    fn expired_identity_budget_and_invalid_pids_do_not_query_a_process() {
        let expired = Query::new(Instant::now());
        assert_eq!(signed_code(&expired, 0), Err(Reason::ProbeTimeout));
        assert_eq!(capture(&expired, 0), Err(Reason::ProbeTimeout));
        for pid in [0, -1] {
            assert_eq!(signed_code(&query(), pid), Err(Reason::TargetUnavailable));
            assert_eq!(capture(&query(), pid), Err(Reason::TargetUnavailable));
        }
    }

    #[test]
    fn kernel_start_time_identifies_a_launch_without_launch_services() {
        let pid = std::process::id() as i32;
        let start = started(pid).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert!(start > now - 86_400.0 && start <= now + 1.0);
        assert_eq!(started(pid).unwrap().to_bits(), start.to_bits());
        assert_eq!(started(-1), Err(Reason::TargetUnavailable));
        assert_eq!(std::mem::size_of::<BsdInfo>(), 136);
    }

    #[test]
    fn native_requirement_rejects_this_non_textedit_process() {
        autoreleasepool(|_| {
            assert_eq!(
                signed_code(&rejection_query(), std::process::id() as i32),
                Err(Reason::UnsupportedTarget)
            );
        });
    }

    #[test]
    #[ignore = "read-only identity check; requires exactly one already running Apple TextEdit"]
    fn native_running_textedit_identity() {
        autoreleasepool(|_| {
            let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
                &NSString::from_str(BUNDLE),
            );
            assert_eq!(
                apps.count(),
                1,
                "Open one Apple TextEdit before this check; no app is launched by the test"
            );
            let pid = apps.objectAtIndex(0).processIdentifier();
            let start = Instant::now();
            let first = capture(&query(), pid).unwrap();
            let first_ms = start.elapsed().as_secs_f64() * 1000.0;
            let mut maximum = 0.0_f64;
            for _ in 0..12 {
                let start = Instant::now();
                assert_eq!(capture(&query(), pid).unwrap(), first);
                maximum = maximum.max(start.elapsed().as_secs_f64() * 1000.0);
            }
            println!("TextEdit version={} build={} initial_ms={first_ms:.2} repeat_max_ms={maximum:.2}; signature identity only, no AX or delivery qualification", first.code.as_ref().unwrap().version, first.code.as_ref().unwrap().build);
        });
    }

    #[test]
    #[ignore = "creates and runs one temporary ad-hoc signed sleep process; never opens an app or reads AX"]
    fn native_adhoc_claiming_textedit_is_rejected() {
        use std::process::{Child, Command, Stdio};
        struct FixtureProcess(Child);
        impl Drop for FixtureProcess {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("candidate");
        std::fs::copy("/bin/sleep", &path).unwrap();
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--identifier", BUNDLE])
            .arg(&path)
            .output()
            .unwrap();
        assert!(signed.status.success(), "sign only our temporary fixture");
        let process = FixtureProcess(
            Command::new(&path)
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        autoreleasepool(|_| {
            assert_eq!(
                signed_code(&rejection_query(), process.0.id() as i32),
                Err(Reason::UnsupportedTarget)
            );
        });
        // RAII terminates/reaps only this fixture, including on assertion failure.
    }
}
